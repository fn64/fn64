//! Fast, path-free ROM identity receipt for external-tool workspace keys.

use fn64_discover::normalize;
use serde::Serialize;
use std::path::PathBuf;

const MAX_ROM_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Serialize)]
struct RomIdentityReceipt<'a> {
    schema: &'static str,
    schema_version: u32,
    normalized_rom_sha256: &'a str,
    source_byte_order: String,
    byte_length: usize,
    entry_point: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("rom-identity: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "usage: rom_identity ROM".to_string())?;
    if args.next().is_some() {
        return Err("usage: rom_identity ROM".into());
    }
    let bytes = read_rom_bounded(&path)?;
    let rom = normalize(&bytes).map_err(|error| format!("normalizing ROM: {error:?}"))?;
    let receipt = RomIdentityReceipt {
        schema: "fn64.rom-identity",
        schema_version: 1,
        normalized_rom_sha256: &rom.sha256,
        source_byte_order: rom.source_byte_order.to_string(),
        byte_length: rom.len(),
        entry_point: rom.header.entry_point,
    };
    println!(
        "{}",
        serde_json::to_string(&receipt)
            .map_err(|error| format!("serializing ROM identity: {error}"))?
    );
    Ok(())
}

fn read_rom_bounded(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("inspecting ROM {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("ROM is not a regular file: {}", path.display()));
    }
    if metadata.len() > MAX_ROM_BYTES {
        return Err(format!("ROM exceeds {MAX_ROM_BYTES} bytes"));
    }
    let bytes =
        std::fs::read(path).map_err(|error| format!("reading ROM {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_ROM_BYTES {
        return Err(format!(
            "ROM grew beyond {MAX_ROM_BYTES} bytes while reading"
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn oversized_sparse_rom_is_rejected_before_allocation() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "fn64-rom-identity-oversized-{}-{nonce}",
            std::process::id()
        ));
        let file = File::create(&path).unwrap();
        file.set_len(MAX_ROM_BYTES + 1).unwrap();
        drop(file);
        let error = read_rom_bounded(&path).unwrap_err();
        assert!(error.contains("ROM exceeds"), "{error}");
        std::fs::remove_file(path).unwrap();
    }
}
