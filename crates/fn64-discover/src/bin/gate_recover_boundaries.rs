//! Answer-key-free function-boundary recovery — emit the list.
//!
//! Prints the function starts (+ proven byte extents where available) for a
//! ROM's boot bank AND its mechanically-recovered overlay banks, using ONLY
//! ROM bytes and discovery facts (no decomp answer key). The recovery lives in
//! `fn64_discover::boundaries`; this binary is the thin emitter.
//!
//! Output: JSON-lines, one `{bank, entry, va_end?, bytes?, exact}` per
//! boundary. A summary goes to stderr.
//!
//! Env:  FN64_DISCOVER_ROM   the game's .z64  (AKI family; no answer key)

use fn64_discover::boundaries::recover_boundaries;
use fn64_discover::required_env_path;

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_recover_boundaries: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rom_path =
        required_env_path("FN64_DISCOVER_ROM", "the game's .z64").map_err(|e| e.to_string())?;
    let rom_bytes =
        std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;

    let program = recover_boundaries(&rom_bytes)?;

    let with_extent = program
        .boundaries
        .iter()
        .filter(|b| b.va_end.is_some())
        .count();
    eprintln!(
        "recover-boundaries: {} banks, {} function boundaries ({} with a proven byte extent)",
        program.banks.len(),
        program.boundaries.len(),
        with_extent
    );
    for b in &program.boundaries {
        match b.va_end {
            Some(end) => println!(
                "{{\"bank\":\"{}\",\"entry\":\"0x{:x}\",\"va_end\":\"0x{:x}\",\"bytes\":{},\"exact\":true}}",
                b.bank, b.entry, end, end.saturating_sub(b.entry)
            ),
            None => println!(
                "{{\"bank\":\"{}\",\"entry\":\"0x{:x}\",\"exact\":false}}",
                b.bank, b.entry
            ),
        }
    }
    Ok(())
}
