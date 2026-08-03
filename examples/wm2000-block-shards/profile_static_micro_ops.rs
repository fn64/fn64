//! Content-silent whole-WM static-micro-op size and identity probe.
//!
//! ROM-derived records remain in memory. Standard output contains only fixed
//! counts, a digest over per-package identities, and the format's size gate.

#[allow(dead_code)]
#[path = "build.rs"]
mod generator;

use std::path::PathBuf;

use sha2::{Digest, Sha256};

const SCHEMA: &str = "fn64.wm-static-micro-op-profile.v2";
const COMPLETE_PACK_CEILING_BYTES: u64 = 12 * 1024 * 1024;

fn main() {
    if let Err(error) = run() {
        eprintln!("WM static-micro-op profile: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom = parse_rom_argument()?;
    let source = std::fs::read(rom).map_err(|_| "cannot read private ROM input".to_owned())?;
    let mut generator = generator::WmShardGenerator::from_rom_bytes(&source);
    let mut total_bytes = 0u64;
    let mut total_instructions = 0u64;
    let mut inventory = Sha256::new();
    inventory.update(b"fn64:wm-static-micro-op-profile:v2:");

    for package in generator::PACKAGES {
        let profile = generator.profile_static_micro_ops(package);
        let package_bytes = u64::try_from(profile.bytes)
            .map_err(|_| "static-micro-op package byte count does not fit u64")?;
        total_bytes = total_bytes
            .checked_add(package_bytes)
            .ok_or_else(|| "static-micro-op byte inventory overflow".to_owned())?;
        total_instructions = total_instructions
            .checked_add(profile.instructions)
            .ok_or_else(|| "static-micro-op instruction inventory overflow".to_owned())?;
        inventory.update((package.len() as u64).to_be_bytes());
        inventory.update(package.as_bytes());
        inventory.update(package_bytes.to_be_bytes());
        inventory.update(profile.instructions.to_be_bytes());
        inventory.update(profile.body_sha256);
    }

    if total_bytes >= COMPLETE_PACK_CEILING_BYTES {
        return Err(format!(
            "static-micro-op inventory is {total_bytes} bytes, not below the {COMPLETE_PACK_CEILING_BYTES}-byte gate"
        ));
    }
    let inventory_sha256: [u8; 32] = inventory.finalize().into();
    println!(
        "schema={SCHEMA} packages={} instructions={total_instructions} bytes={total_bytes} ceiling_bytes={COMPLETE_PACK_CEILING_BYTES} inventory_sha256={}",
        generator::PACKAGES.len(),
        hex(inventory_sha256),
    );
    Ok(())
}

fn parse_rom_argument() -> Result<PathBuf, String> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--rom")) {
        return Err("usage: fn64-wm-static-micro-op-profile --rom ABSOLUTE_PATH".to_owned());
    }
    let rom = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| "--rom requires one path".to_owned())?,
    );
    if arguments.next().is_some() {
        return Err("unexpected trailing argument".to_owned());
    }
    if !rom.is_absolute() {
        return Err("--rom must be an absolute path".to_owned());
    }
    Ok(rom)
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
    fn digest_format_is_canonical_lowercase_hex() {
        assert_eq!(hex([0xab; 32]), "ab".repeat(32));
    }
}
