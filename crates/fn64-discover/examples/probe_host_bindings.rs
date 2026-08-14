//! Which WM-block runtime host symbols does a ROM resolve uniquely?
//!
//! The shard builder requires ALL of them, which is what stops the boot lane
//! generalizing past WM2000. This reports the per-symbol outcome so the gap is
//! sized rather than guessed at.
fn main() {
    let path = std::env::args().nth(1).expect("usage: <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let rom = fn64_discover::rom::normalize(&bytes).expect("normalize");
    let label = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    const ROM_START: usize = 0x1000;
    const BOOT_BYTES: usize = 0x100000;
    const VA_START: u32 = 0x8000_0400;
    if rom.bytes.len() < ROM_START + BOOT_BYTES {
        println!("{label}\tROM_TOO_SMALL");
        return;
    }
    let signature = rom.bytes[ROM_START..ROM_START + BOOT_BYTES]
        .chunks_exact(4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .collect::<Vec<_>>();
    let total = fn64_discover::host_bindings::WM_BLOCK_RUNTIME_HOST_SYMBOLS.len();
    match fn64_discover::host_bindings::discover_wm_block_runtime_host_bindings(
        &signature, VA_START,
    ) {
        Ok(found) => println!("{label}\tOK\t{}/{total}", found.len()),
        Err(error) => println!("{label}\tFAIL\t{total}\t{error:?}"),
    }
}
