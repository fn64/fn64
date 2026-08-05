//! Probe: does the 64DD drive-init recognizer find the routine, and where?
use fn64_discover::host_bindings::discover_drive_rom_init_host_binding;

fn main() {
    let path = std::env::args().nth(1).expect("usage: <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let rom = fn64_discover::rom::normalize(&bytes).expect("normalize");
    // The boot bank: ROM 0x1000 maps to the header entry point.
    let entry = rom.header.entry_point;
    let words: Vec<u32> = rom.bytes[0x1000..0x101000]
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
        .collect();
    match discover_drive_rom_init_host_binding(&words, entry) {
        Ok(Some(found)) => println!(
            "FOUND at {:#010x} guard={:#010x}",
            found.binding.vram, found.guard_vram
        ),
        Ok(None) => println!("absent (no disk-drive routine)"),
        Err(error) => println!("ERR {error:?}"),
    }
}
