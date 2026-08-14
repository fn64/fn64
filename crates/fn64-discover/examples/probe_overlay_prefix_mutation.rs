//! Does WM2000's `0x800e1b90` overlay carry a mutable non-code prefix?
//!
//! Running the certified route past the overlay entry stops with:
//!
//! ```text
//! generation activation at 0x800F61B4 failed:
//! AotMiss for bank:89C4A396A5B53C23 range 0x800E1B90..0x80100400
//! expected 5066618c... observed 2fb3f01e...
//! ```
//!
//! `activate_for_fetch_with_digest` (`fn64-recomp-rs/src/generation/mod.rs:723`)
//! digests EVERY generation containing the PC and admits the one that matches,
//! so two overlays sharing a base is already handled. `matches` came back
//! EMPTY, meaning no compiled generation matched the bytes in RAM -- including
//! the longer one the guest had actually entered.
//!
//! `docs/UNIVERSAL-RUNTIME-PLAN.md:605-612` records why: this image's "first
//! `0x790` bytes are mutable non-code state", and only the suffix
//! `0x800e1b90..0x800e2400` is byte-identical to its ROM mapping. A generation
//! hashed from `image_start` therefore includes bytes the game writes at
//! runtime, and any such write breaks the digest for every generation at that
//! base.
//!
//! This checks that claim against the ROM rather than trusting the note: it
//! compares the overlay's ROM-resident bytes against the region the prior
//! analysis calls byte-identical, and reports where code actually starts.

fn main() {
    let path = std::env::args().nth(1).expect("usage: <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let rom = fn64_discover::rom::normalize(&bytes).expect("normalize");

    // From docs/UNIVERSAL-RUNTIME-PLAN.md:610 -- the executable suffix
    // 0x800e1b90..0x800e2400 is byte-identical to ROM 0xe2790..0xe3000.
    const ROM_SUFFIX_START: usize = 0xe2790;
    const ROM_SUFFIX_END: usize = 0xe3000;
    const CLAIMED_PREFIX_LEN: usize = 0x790;

    println!("overlay base 0x800e1b90, claimed mutable prefix {CLAIMED_PREFIX_LEN:#x} bytes");
    println!(
        "prior note: executable suffix maps ROM [{ROM_SUFFIX_START:#x},{ROM_SUFFIX_END:#x})"
    );

    // Where does executable content actually begin? `jr ra` density per 256
    // bytes distinguishes a data prefix from code.
    let window = 0x100;
    let scan_start = ROM_SUFFIX_START.saturating_sub(CLAIMED_PREFIX_LEN);
    println!("\nscanning ROM from {scan_start:#x} in {window:#x}-byte windows:");
    let mut first_code_window = None;
    let mut offset = scan_start;
    while offset + window <= ROM_SUFFIX_END.max(scan_start + 0x1000) && offset + window <= rom.bytes.len() {
        let sites = (offset..offset + window)
            .step_by(4)
            .filter(|&at| rom.bytes[at..at + 4] == [0x03, 0xe0, 0x00, 0x08])
            .count();
        // Also count plausible MIPS opcodes as a weaker code signal.
        let plausible = (offset..offset + window)
            .step_by(4)
            .filter(|&at| {
                let word = u32::from_be_bytes([
                    rom.bytes[at],
                    rom.bytes[at + 1],
                    rom.bytes[at + 2],
                    rom.bytes[at + 3],
                ]);
                let op = word >> 26;
                // lui/addiu/lw/sw/beq/bne/jal/j/special -- the common encodings.
                matches!(op, 0x00 | 0x02 | 0x03 | 0x04 | 0x05 | 0x08 | 0x09 | 0x0f | 0x23 | 0x2b)
            })
            .count();
        let marker = if plausible >= 48 { "CODE" } else { "data" };
        if plausible >= 48 && first_code_window.is_none() {
            first_code_window = Some(offset);
        }
        println!(
            "  rom {offset:#08x}  jr_ra={sites:2}  plausible_ops={plausible:2}/64  {marker}"
        );
        offset += window;
    }

    match first_code_window {
        Some(at) => {
            let delta = at.saturating_sub(scan_start);
            println!("\nfirst code-looking window at ROM {at:#x} (+{delta:#x} from scan start)");
            println!(
                "claimed prefix is {CLAIMED_PREFIX_LEN:#x}; measured leading non-code is {delta:#x}"
            );
        }
        None => println!("\nno code-looking window found in the scanned range"),
    }
}
