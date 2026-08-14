//! One-shot producer for a private WM prepared-shard source tree.
//!
//! This process never executes ROM code. It performs the same static
//! discovery and emission as the legacy shard build, once for every shard
//! in the shared `shard_inventory.in` catalog.

#[allow(dead_code)]
#[path = "build.rs"]
mod generator;
#[cfg(test)]
#[path = "materializer.rs"]
mod materializer;
mod prepared_tree;

use std::path::PathBuf;

use prepared_tree::SourceIdentityClaims;

struct Arguments {
    rom: PathBuf,
    output: PathBuf,
    claims: SourceIdentityClaims,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("WM prepared-shard producer: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = parse_arguments()?;
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|_| "cannot resolve repository boundary".to_owned())?;
    if !repository.join("Cargo.toml").is_file() || !repository.join("AGENTS.md").is_file() {
        return Err("resolved repository boundary lacks required sentinels".to_owned());
    }
    let source =
        std::fs::read(&arguments.rom).map_err(|_| "cannot read private ROM input".to_owned())?;
    let mut generator = generator::WmShardGenerator::from_rom_bytes(&source);
    let normalized_rom_sha256 = generator.normalized_rom_sha256();
    let mut publication = prepared_tree::PreparedTreePublication::begin(
        &arguments.output,
        &repository,
        normalized_rom_sha256,
        arguments.claims,
    )
    .map_err(|error| error.to_string())?;
    for package in generator::PACKAGES {
        publication
            .push(generator.generate_package(package))
            .map_err(|error| error.to_string())?;
    }
    let prepared_tree_sha256 = publication.finish().map_err(|error| error.to_string())?;
    println!(
        "schema={} normalized_rom_sha256={} prepared_manifest_sha256={}",
        prepared_tree::ROOT_SCHEMA_V2,
        hex(normalized_rom_sha256),
        hex(prepared_tree_sha256),
    );
    Ok(())
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut values = std::env::args_os().skip(1);
    let mut rom = None;
    let mut output = None;
    let mut generator_source_sha256 = None;
    let mut discovery_source_sha256 = None;
    let mut emitter_source_sha256 = None;
    let mut runtime_source_sha256 = None;
    while let Some(flag) = values.next() {
        let flag = flag
            .into_string()
            .map_err(|_| "argument flag is not UTF-8".to_owned())?;
        let value = values
            .next()
            .ok_or_else(|| format!("{flag} requires one value"))?;
        match flag.as_str() {
            "--rom" => set_once(&mut rom, PathBuf::from(value), "--rom")?,
            "--output" => set_once(&mut output, PathBuf::from(value), "--output")?,
            "--generator-source-sha256" => set_once(
                &mut generator_source_sha256,
                parse_digest(value, "--generator-source-sha256")?,
                "--generator-source-sha256",
            )?,
            "--discovery-source-sha256" => set_once(
                &mut discovery_source_sha256,
                parse_digest(value, "--discovery-source-sha256")?,
                "--discovery-source-sha256",
            )?,
            "--emitter-source-sha256" => set_once(
                &mut emitter_source_sha256,
                parse_digest(value, "--emitter-source-sha256")?,
                "--emitter-source-sha256",
            )?,
            "--runtime-source-sha256" => set_once(
                &mut runtime_source_sha256,
                parse_digest(value, "--runtime-source-sha256")?,
                "--runtime-source-sha256",
            )?,
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    Ok(Arguments {
        rom: rom.ok_or_else(|| "missing --rom".to_owned())?,
        output: output.ok_or_else(|| "missing --output".to_owned())?,
        claims: SourceIdentityClaims {
            generator_source_sha256: generator_source_sha256
                .ok_or_else(|| "missing --generator-source-sha256".to_owned())?,
            discovery_source_sha256: discovery_source_sha256
                .ok_or_else(|| "missing --discovery-source-sha256".to_owned())?,
            emitter_source_sha256: emitter_source_sha256
                .ok_or_else(|| "missing --emitter-source-sha256".to_owned())?,
            runtime_source_sha256: runtime_source_sha256
                .ok_or_else(|| "missing --runtime-source-sha256".to_owned())?,
        },
    })
}

fn set_once<T>(slot: &mut Option<T>, value: T, label: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("duplicate {label}"));
    }
    Ok(())
}

fn parse_digest(value: std::ffi::OsString, label: &str) -> Result<[u8; 32], String> {
    let value = value
        .into_string()
        .map_err(|_| format!("{label} is not UTF-8"))?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format!("{label} must be lowercase SHA-256"));
    }
    let mut digest = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        digest[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    if digest == [0; 32] {
        return Err(format!("{label} must be nonzero"));
    }
    Ok(digest)
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("validated lowercase hex"),
    }
}

fn hex(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_parser_is_canonical_and_nonzero() {
        assert_eq!(
            parse_digest("11".repeat(32).into(), "digest").unwrap(),
            [0x11; 32]
        );
        for rejected in ["00".repeat(32), "AA".repeat(32), "1".repeat(63)] {
            assert!(parse_digest(rejected.into(), "digest").is_err());
        }
    }
}
