//! Corpus-wide completeness guard for the MIPS decoder.
//!
//! `isa_completeness.rs` proves the decoder against hand-cited encodings; this
//! proves it against the encodings real N64 code actually contains, by sweeping
//! every `.z64` in a local corpus directory and decoding the bodies of every
//! prologue-delimited function in each ROM's boot copy.
//!
//! The pinned property is a RATE, not a count: `Unknown` must stay a rounding
//! error. Measured across 287 ROMs: 23,808,397 words decoded, 289 `Unknown`,
//! 0.0012%. The residual clusters on reserved primary opcodes (0x13, 0x1c-0x1f,
//! 0x33, 0x3b) -- embedded data inside function extents, not instructions the
//! decoder is missing. A regression that stops decoding a real encoding moves
//! the rate by orders of magnitude, and the failure histogram names the opcode.
//!
//! Note 0x3e is NOT reserved here: it decodes as `Sd`. An independent estimate
//! that treated it as reserved reported a ~65x higher residual, all of it that
//! estimate's own missing coverage rather than this decoder's.
//!
//! ROM-gated by `FN64_RECOMP_SWEEP_DIR`. An unset var is a loud skip; a set var
//! that names an unreadable directory or a malformed ROM is a failure. No ROM
//! path is hardcoded and no ROM bytes enter this repository.

use std::collections::BTreeMap;

use fn64_cpu_runtime::{decode, Instruction};

/// Pinned ceiling on the aggregate `Unknown` rate. Measured 0.0012% across the
/// full 287-ROM corpus; 0.01% leaves ~8x headroom for corpus composition while
/// still catching a decoder that lost an encoding class. A dropped encoding
/// class moves the rate by orders of magnitude, not by a factor of eight.
const UNKNOWN_RATE_CEILING: f64 = 0.0001;

/// The PI DMA the boot sequence performs before jumping to the entrypoint:
/// ROM `[0x1000, 0x101000)` is copied to RDRAM. Hardware constant, identical
/// for every commercial cartridge, so it needs no per-ROM discovery.
const BOOT_COPY_OFFSET: usize = 0x1000;
const BOOT_COPY_LEN: usize = 0x100_000;

/// `jr $ra` -- the only function terminator this sweep accepts.
const JR_RA: u32 = 0x03E0_0008;

/// Functions longer than this are assumed to be a prologue match that never
/// found its real terminator, so the body is not attributed to a function.
const MAX_FUNCTION_WORDS: usize = 400;

#[test]
fn corpus_functions_decode_without_meaningful_unknown_residual() {
    let Some(dir) = std::env::var_os("FN64_RECOMP_SWEEP_DIR") else {
        eprintln!("skip: FN64_RECOMP_SWEEP_DIR unset");
        return;
    };

    let entries = std::fs::read_dir(&dir).unwrap_or_else(|error| {
        panic!("FN64_RECOMP_SWEEP_DIR is set but unreadable ({dir:?}): {error}");
    });
    let mut roms: Vec<std::path::PathBuf> = entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("reading {dir:?}: {error}"))
                .path()
        })
        .filter(|path| path.extension().is_some_and(|ext| ext == "z64"))
        .collect();
    roms.sort();
    assert!(
        !roms.is_empty(),
        "FN64_RECOMP_SWEEP_DIR ({dir:?}) contains no .z64 ROMs"
    );

    let mut total_words = 0usize;
    let mut total_unknown = 0usize;
    let mut histogram: BTreeMap<u32, usize> = BTreeMap::new();

    for rom in &roms {
        let bytes = std::fs::read(rom).unwrap_or_else(|error| {
            panic!("{rom:?} is in FN64_RECOMP_SWEEP_DIR but unreadable: {error}");
        });
        let end = BOOT_COPY_OFFSET + BOOT_COPY_LEN;
        assert!(
            bytes.len() >= end,
            "{rom:?} is {} bytes; a .z64 cartridge image must contain the whole \
             boot copy through {end:#x}",
            bytes.len()
        );
        let words: Vec<u32> = bytes[BOOT_COPY_OFFSET..end]
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        let (rom_words, rom_unknown) = sweep(&words, &mut histogram);
        total_words += rom_words;
        total_unknown += rom_unknown;
        eprintln!(
            "{:>10} words {:>6} unknown  {}",
            rom_words,
            rom_unknown,
            rom.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    assert!(
        total_words > 0,
        "no function bodies were found across {} ROM(s); the prologue scan is \
         not selecting anything and the sweep proves nothing",
        roms.len()
    );

    let rate = total_unknown as f64 / total_words as f64;
    eprintln!(
        "corpus decode sweep: {} ROM(s), {total_words} words, {total_unknown} unknown ({:.4}%)",
        roms.len(),
        rate * 100.0
    );

    if rate >= UNKNOWN_RATE_CEILING {
        let mut residuals: Vec<(u32, usize)> = histogram.into_iter().collect();
        residuals.sort_by_key(|&(opcode, count)| (std::cmp::Reverse(count), opcode));
        eprintln!("Unknown residuals by primary opcode:");
        for (opcode, count) in residuals {
            eprintln!("  op {opcode:#04x}: {count}");
        }
        panic!(
            "Unknown decode rate {:.4}% exceeds the pinned {:.4}% ceiling \
             ({total_unknown}/{total_words}); see the per-opcode histogram above \
             for the encoding that stopped decoding",
            rate * 100.0,
            UNKNOWN_RATE_CEILING * 100.0
        );
    }
}

/// Decodes every word of every `addiu sp,sp,-N` ... `jr $ra` extent in `words`,
/// returning (decoded, unknown) and accumulating unknown primary opcodes.
fn sweep(words: &[u32], histogram: &mut BTreeMap<u32, usize>) -> (usize, usize) {
    let mut decoded = 0usize;
    let mut unknown = 0usize;
    let mut index = 0usize;

    while index < words.len() {
        if !is_stack_prologue(words[index]) {
            index += 1;
            continue;
        }
        let limit = (index + MAX_FUNCTION_WORDS).min(words.len());
        let Some(offset) = words[index..limit].iter().position(|&w| w == JR_RA) else {
            index += 1;
            continue;
        };

        // The delay slot after `jr $ra` belongs to the function.
        let body_end = (index + offset + 2).min(words.len());
        for &word in &words[index..body_end] {
            decoded += 1;
            if matches!(decode(word), Instruction::Unknown { .. }) {
                unknown += 1;
                *histogram.entry(word >> 26).or_default() += 1;
            }
        }
        index = body_end;
    }

    (decoded, unknown)
}

/// `addiu sp, sp, -N`: primary opcode 0x09 with rs == rt == $sp and a negative
/// immediate. The stack-allocating prologue that opens a non-leaf function.
fn is_stack_prologue(word: u32) -> bool {
    word >> 26 == 0x09
        && (word >> 21) & 0x1F == 29
        && (word >> 16) & 0x1F == 29
        && word & 0x8000 != 0
}
