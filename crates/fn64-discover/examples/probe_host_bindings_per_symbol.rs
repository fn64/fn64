//! Per-symbol host-binding matrix: run every recognizer independently.
//!
//! `probe_host_bindings` reports the production chain, which is a chain of `?`
//! and therefore names only the symbol it stopped at. It says nothing about
//! the roles behind that one, and on failure it prints the constant 15 -- the
//! symbol count, not a score. That has been misread as a score.
//!
//! This probe runs each recognizer standalone so nothing hides behind the
//! short-circuit, and distinguishes three outcomes that the chain conflates:
//! resolved, not-resolved (the recognizer ran and found nothing, or found
//! several), and not-reached (the recognizer never ran because it is only
//! reachable through an earlier stage's output).
//!
//! Every path prints both numerator and denominator.
//!
//! Usage:
//!     cargo run --release -p fn64-discover \
//!         --example probe_host_bindings_per_symbol -- <rom.z64> [more.z64 ...]
use fn64_discover::host_bindings::{
    HostBindingProbeOutcome, HostBindingSymbol, WM_BLOCK_RUNTIME_HOST_SYMBOLS,
};

const ROM_START: usize = 0x1000;
const BOOT_BYTES: usize = 0x100000;
const VA_START: u32 = 0x8000_0400;

/// The nine roles the earlier published matrix scored, kept as a named subset
/// so this probe's numbers stay comparable to the figures already on record.
const LEGACY_NINE: [HostBindingSymbol; 9] = [
    HostBindingSymbol::OsCreateMesgQueue,
    HostBindingSymbol::OsEPiStartDma,
    HostBindingSymbol::OsGetThreadPri,
    HostBindingSymbol::OsSendMesg,
    HostBindingSymbol::OsSetEventMesg,
    HostBindingSymbol::OsSetThreadPri,
    HostBindingSymbol::OsStartThread,
    HostBindingSymbol::OsSiDeviceBusy,
    HostBindingSymbol::OsSetTimer,
];

fn symbol_name(symbol: HostBindingSymbol) -> &'static str {
    match symbol {
        HostBindingSymbol::OsCreateMesgQueue => "osCreateMesgQueue",
        HostBindingSymbol::OsCreateThread => "osCreateThread",
        HostBindingSymbol::OsDriveRomInit => "osDriveRomInit",
        HostBindingSymbol::OsEPiStartDma => "osEPiStartDma",
        HostBindingSymbol::OsGetThreadPri => "osGetThreadPri",
        HostBindingSymbol::OsRecvMesg => "osRecvMesg",
        HostBindingSymbol::OsSendMesg => "osSendMesg",
        HostBindingSymbol::OsSetEventMesg => "osSetEventMesg",
        HostBindingSymbol::OsSiDeviceBusy => "__osSiDeviceBusy",
        HostBindingSymbol::OsSetThreadPri => "osSetThreadPri",
        HostBindingSymbol::OsSetTimer => "osSetTimer",
        HostBindingSymbol::OsSpTaskLoad => "osSpTaskLoad",
        HostBindingSymbol::OsSpTaskStartGo => "osSpTaskStartGo",
        HostBindingSymbol::OsSpTaskYield => "osSpTaskYield",
        HostBindingSymbol::OsSpTaskYielded => "osSpTaskYielded",
        HostBindingSymbol::OsStartThread => "osStartThread",
    }
}

fn cell(outcome: &HostBindingProbeOutcome) -> String {
    match outcome {
        HostBindingProbeOutcome::Resolved { vram } => format!("1 @{vram:08x}"),
        HostBindingProbeOutcome::Absent => "0 absent".to_string(),
        HostBindingProbeOutcome::Ambiguous { candidates } => {
            format!("0 ambiguous({})", candidates.len())
        }
        HostBindingProbeOutcome::Failed { detail } => format!("0 failed({detail})"),
        HostBindingProbeOutcome::NotReached { needs } => format!("- not-reached(needs {needs})"),
    }
}

struct ProbedRom {
    label: String,
    outcomes: Vec<(HostBindingSymbol, HostBindingProbeOutcome)>,
}

fn main() {
    let paths = std::env::args().skip(1).collect::<Vec<_>>();
    if paths.is_empty() {
        eprintln!("usage: <rom.z64> [more.z64 ...]");
        std::process::exit(2);
    }

    let mut probed = Vec::new();
    for path in &paths {
        let label = std::path::Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let Ok(bytes) = std::fs::read(path) else {
            println!("{label}\tREAD_FAILED\t{path}");
            continue;
        };
        let Ok(rom) = fn64_discover::rom::normalize(&bytes) else {
            println!("{label}\tNORMALIZE_FAILED");
            continue;
        };
        if rom.bytes.len() < ROM_START + BOOT_BYTES {
            println!("{label}\tROM_TOO_SMALL");
            continue;
        }
        let words = rom.bytes[ROM_START..ROM_START + BOOT_BYTES]
            .chunks_exact(4)
            .map(|word| u32::from_be_bytes(word.try_into().expect("four bytes")))
            .collect::<Vec<_>>();
        let outcomes = fn64_discover::host_bindings::probe_wm_block_runtime_host_bindings(
            &words, VA_START,
        );
        probed.push(ProbedRom { label, outcomes });
    }

    if probed.is_empty() {
        return;
    }

    // Per-ROM detail.
    for rom in &probed {
        println!("== {} ==", rom.label);
        for (symbol, outcome) in &rom.outcomes {
            println!("  {:<18} {}", symbol_name(*symbol), cell(outcome));
        }
        let all = WM_BLOCK_RUNTIME_HOST_SYMBOLS.len();
        let evaluated = rom
            .outcomes
            .iter()
            .filter(|(_, outcome)| outcome.was_evaluated())
            .count();
        let resolved = rom
            .outcomes
            .iter()
            .filter(|(_, outcome)| outcome.is_resolved())
            .count();
        let legacy_resolved = rom
            .outcomes
            .iter()
            .filter(|(symbol, outcome)| LEGACY_NINE.contains(symbol) && outcome.is_resolved())
            .count();
        // Numerator and denominator on every path, always. A bare count here
        // is what got misread as a score once already.
        println!(
            "  -> resolved {resolved}/{all} of all roles; \
             {resolved}/{evaluated} of roles actually evaluated; \
             {legacy_resolved}/{} on the legacy nine",
            LEGACY_NINE.len()
        );
        println!(
            "     (not-reached: {} -- never evaluated, NOT counted as absent)",
            all - evaluated
        );
        println!();
    }

    // Matrix across ROMs.
    println!("== matrix (1 = resolved, 0 = recognizer ran and did not resolve, - = not reached) ==");
    print!("{:<18}", "symbol");
    for rom in &probed {
        print!(" | {:>14}", truncate(&rom.label, 14));
    }
    println!();
    for symbol in WM_BLOCK_RUNTIME_HOST_SYMBOLS {
        print!("{:<18}", symbol_name(symbol));
        for rom in &probed {
            let mark = rom
                .outcomes
                .iter()
                .find(|(candidate, _)| *candidate == symbol)
                .map(|(_, outcome)| match outcome {
                    HostBindingProbeOutcome::Resolved { .. } => "1",
                    HostBindingProbeOutcome::NotReached { .. } => "-",
                    _ => "0",
                })
                .unwrap_or("?");
            print!(" | {mark:>14}");
        }
        println!();
    }

    for (title, subset) in [
        ("legacy 9", LEGACY_NINE.to_vec()),
        ("all 15", WM_BLOCK_RUNTIME_HOST_SYMBOLS.to_vec()),
    ] {
        print!("{:<18}", format!("{title}"));
        for rom in &probed {
            let resolved = rom
                .outcomes
                .iter()
                .filter(|(symbol, outcome)| subset.contains(symbol) && outcome.is_resolved())
                .count();
            print!(" | {:>14}", format!("{resolved}/{}", subset.len()));
        }
        println!();
    }
    print!("{:<18}", "evaluated only");
    for rom in &probed {
        let evaluated = rom
            .outcomes
            .iter()
            .filter(|(_, outcome)| outcome.was_evaluated())
            .count();
        let resolved = rom
            .outcomes
            .iter()
            .filter(|(_, outcome)| outcome.is_resolved())
            .count();
        print!(" | {:>14}", format!("{resolved}/{evaluated}"));
    }
    println!();
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        text.chars().take(width).collect()
    }
}
