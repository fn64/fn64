//! Fast, path-free ROM identity receipt for external-tool workspace keys.

use fn64_discover::normalize;
use serde::Serialize;
use std::path::PathBuf;

const MAX_ROM_BYTES: u64 = 128 * 1024 * 1024;

/// Every header field fn64 already parses, plus the code-layout class.
///
/// `RomHeader` (`fn64_discover::rom::RomHeader`) decodes eight fields, and
/// until schema 2 this receipt emitted exactly one of them (`entry_point`).
/// The other seven were parsed and discarded on every run, so anyone wanting
/// a ROM's CRC pair, cartridge name, or region byte had to re-parse the header
/// themselves -- from a file this tool had already read and normalized.
///
/// These are recorded, not interpreted. `crc1`/`crc2` are the words IPL3 uses
/// to select a CIC-compatible boot path; fn64 does not re-derive or validate
/// them, and emitting them makes no claim that they are correct.
#[derive(Serialize)]
struct RomIdentityReceipt<'a> {
    schema: &'static str,
    schema_version: u32,
    normalized_rom_sha256: &'a str,
    source_byte_order: String,
    byte_length: usize,
    entry_point: u32,
    /// PI BSD domain 1 config word at header offset 0x00.
    pi_bsd_dom1_config: u32,
    /// Clock rate override at 0x04. Zero means "use the default".
    clock_rate: u32,
    /// Libultra revision at 0x0c.
    libultra_version: u32,
    /// CRC1/CRC2 at 0x10/0x14, recorded verbatim, never re-derived.
    crc1: u32,
    crc2: u32,
    /// ASCII cartridge name from 0x20..0x34, trailing NUL/space stripped.
    name: &'a str,
    /// The four bytes at 0x3b..0x3f -- media format, two-character cartridge
    /// id, and region -- emitted individually so no consumer has to slice a
    /// blob. Emitted as chars because they are ASCII by convention: WM2000 is
    /// media 'N', id "WX", region 'E'.
    media_format: char,
    cartridge_id: String,
    region: char,
    /// Where executable code sits: one resident bank or several spans.
    ///
    /// This is the fact that separates "this ROM has no overlays, so finding
    /// none is COMPLETE" from "this ROM has overlays we did not find". It is a
    /// permanent property of the image, so it belongs beside the header rather
    /// than in a discovery outcome, which measures the current build instead.
    code_span_class: String,
    code_span_jr_ra_sites: usize,
    code_span_count: usize,
    code_span_largest_concentration: f64,
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
    let locality = fn64_discover::code_span_locality::measure_code_span_locality(&rom.bytes);
    // 0x3b..0x3f is [media format, id hi, id lo, region] -- e.g. WM2000's
    // b"NWXE" is media 'N' (cartridge), id "WX", region 'E' (USA). The struct
    // doc calls the last byte "version", but `rom.rs:270` asserts b"CTSE" for
    // a USA ROM, so the fourth byte is the REGION code.
    let [media_format, cartridge_id_hi, cartridge_id_lo, region] = rom.header.cartridge_id;
    let receipt = RomIdentityReceipt {
        schema: "fn64.rom-identity",
        // v2 adds the seven parsed-but-discarded header fields and the
        // code-span class. Purely additive: every v1 field keeps its name,
        // type, and meaning.
        schema_version: 2,
        normalized_rom_sha256: &rom.sha256,
        source_byte_order: rom.source_byte_order.to_string(),
        byte_length: rom.len(),
        entry_point: rom.header.entry_point,
        pi_bsd_dom1_config: rom.header.pi_bsd_dom1_config,
        clock_rate: rom.header.clock_rate,
        libultra_version: rom.header.libultra_version,
        crc1: rom.header.crc1,
        crc2: rom.header.crc2,
        name: rom.header.name.as_str(),
        media_format: media_format as char,
        cartridge_id: [cartridge_id_hi as char, cartridge_id_lo as char]
            .iter()
            .collect(),
        region: region as char,
        code_span_class: format!("{:?}", locality.class),
        code_span_jr_ra_sites: locality.jr_ra_sites,
        code_span_count: locality.span_count,
        code_span_largest_concentration: locality.largest_span_concentration,
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
