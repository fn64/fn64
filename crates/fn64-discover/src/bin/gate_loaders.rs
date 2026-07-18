//! Grading-only real-ROM gate for the generic hardware-entry stub recognizer.
//! Expected main entries are consulted only after recognition.

use fn64_discover::loaders::{recognize_entry_stub_any, RecognizedEntryStub, VirtualAddress};
use fn64_discover::normalize;

fn main() {
    let nw4e_rom = required_path("FN64_DISCOVER_NW4E_ROM");
    let nwxe_rom = required_path("FN64_DISCOVER_NWXE_ROM");
    let (Ok(nw4e_rom), Ok(nwxe_rom)) = (nw4e_rom, nwxe_rom) else {
        eprintln!("gate_loaders: FN64_DISCOVER_NW4E_ROM and FN64_DISCOVER_NWXE_ROM are required");
        std::process::exit(1);
    };
    let mut failed = false;
    for (label, path, expected_main) in [
        ("NW4E", nw4e_rom.as_str(), 0x8000_0460),
        ("NWXE", nwxe_rom.as_str(), 0x8000_0460),
    ] {
        match grade_one(label, path, expected_main) {
            Ok(()) => {}
            Err(error) => {
                eprintln!("{label}: {error}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}

fn required_path(variable: &str) -> Result<String, std::env::VarError> {
    std::env::var(variable)
}

fn grade_one(label: &str, path: &str, expected_main: u32) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("reading {path}: {error}"))?;
    let rom = normalize(&bytes).map_err(|error| error.to_string())?;
    let boot = rom
        .bytes
        .get(0x1000..)
        .ok_or_else(|| "ROM has no hardware boot-copy source".to_string())?;
    let words: Vec<u32> = boot
        .chunks_exact(4)
        .take(1024)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte chunk")))
        .collect();
    let mut last_rejection = None;
    for word_count in [16usize, 32, 64, 128, 256, 512, 1024] {
        match recognize_entry_stub_any(
            &words[..word_count.min(words.len())],
            VirtualAddress::new(rom.header.entry_point),
        ) {
            Ok(RecognizedEntryStub::Countdown(observation)) => {
                let jump = observation.post_clear_constructed_jump.ok_or_else(|| {
                    "recognized countdown zero-fill has no constructed jump".to_string()
                })?;
                println!(
                    "{label}: window_words={word_count} zero_fill=[{:#010x},{:#010x}) stride={} jump={:#010x}->{:#010x}",
                    observation.zero_fill.start.get(),
                    observation.zero_fill.end_exclusive.get(),
                    observation.zero_fill.stride,
                    jump.jump_pc.get(),
                    jump.target.get(),
                );
                if jump.target.get() != expected_main {
                    return Err(format!(
                        "post-clear candidate target {:#010x} differs from held-out expected entry {expected_main:#010x}",
                        jump.target.get()
                    ));
                }
                return Ok(());
            }
            Ok(RecognizedEntryStub::EndPointer(_)) => {
                return Err("expected countdown form but recognized end-pointer form".to_string());
            }
            Err(error) => last_rejection = Some(error),
        }
    }
    Err(format!(
        "no accepted entry stub in 1024-word budget; final rejection: {}",
        last_rejection.expect("at least one window was tested")
    ))
}
