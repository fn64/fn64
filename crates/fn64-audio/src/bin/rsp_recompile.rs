//! `rsp_recompile` — the RSP → typed-Rust recompiler CLI.
//!
//! Reads a raw big-endian RSP ucode text image (e.g. OoT's `aspMainText`
//! incbin), decodes every 32-bit word with the clean-room decoder, and either
//! reports the opcode histogram + any decode gaps, or emits a typed-Rust
//! module implementing the ucode.
//!
//! Usage:
//!   rsp_recompile scan  <ucode_text> [base_vram_hex]
//!   rsp_recompile emit  <ucode_text> <fn_name> [base_vram_hex] > out.rs

use std::collections::BTreeMap;
use std::process::ExitCode;

use fn64_audio::rsp::decode::{decode, Instr};
use fn64_audio::rsp::emit::emit_module;

fn read_words(path: &str) -> Vec<u32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    assert!(bytes.len() % 4 == 0, "ucode text length not word-aligned");
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// A short mnemonic for an instruction, for the histogram / gap report.
fn mnemonic(i: &Instr) -> String {
    match i {
        Instr::Nop => "nop".into(),
        Instr::AluReg { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::Shift { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::ShiftVar { op, .. } => format!("{op:?}v").to_lowercase(),
        Instr::CondMove { on_zero, .. } => {
            if *on_zero {
                "movz".into()
            } else {
                "movn".into()
            }
        }
        Instr::AluImm { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::Lui { .. } => "lui".into(),
        Instr::Load { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::Store { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::Branch { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::BranchZ { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::Jump { .. } => "j".into(),
        Instr::Jal { .. } => "jal".into(),
        Instr::Jr { .. } => "jr".into(),
        Instr::Jalr { .. } => "jalr".into(),
        Instr::Break => "break".into(),
        Instr::Mfc0 { .. } => "mfc0".into(),
        Instr::Mtc0 { .. } => "mtc0".into(),
        Instr::Mfc2 { .. } => "mfc2".into(),
        Instr::Mtc2 { .. } => "mtc2".into(),
        Instr::Cfc2 { .. } => "cfc2".into(),
        Instr::Ctc2 { .. } => "ctc2".into(),
        Instr::VLoad { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::VStore { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::Vu { op, .. } => format!("{op:?}").to_lowercase(),
        Instr::Unknown { .. } => "UNKNOWN".into(),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: rsp_recompile <scan|emit> <ucode_text> [args]");
        return ExitCode::FAILURE;
    }
    let mode = args[1].as_str();
    let path = &args[2];
    let words = read_words(path);

    match mode {
        "scan" => {
            let base = args
                .get(3)
                .map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap())
                .unwrap_or(0x1080);
            let mut hist: BTreeMap<String, usize> = BTreeMap::new();
            let mut unknown: Vec<(u32, u32)> = Vec::new();
            for (i, &w) in words.iter().enumerate() {
                let pc = (base & 0x1FFF) + (i as u32) * 4;
                let instr = decode(w, pc);
                *hist.entry(mnemonic(&instr)).or_default() += 1;
                if let Instr::Unknown { word } = instr {
                    unknown.push((pc, word));
                }
            }
            println!(
                "# {} instructions, base IMEM 0x{:04X}",
                words.len(),
                base & 0x1FFF
            );
            println!("# opcode histogram:");
            for (m, c) in &hist {
                println!("  {m:<8} {c}");
            }
            if unknown.is_empty() {
                println!("# GAPS: none — every word decoded to a known op.");
                ExitCode::SUCCESS
            } else {
                println!("# GAPS: {} undecoded words:", unknown.len());
                for (pc, w) in &unknown {
                    println!("  0x{pc:04X}: 0x{w:08X}");
                }
                ExitCode::FAILURE
            }
        }
        "emit" => {
            let fn_name = args
                .get(3)
                .map(|s| s.as_str())
                .unwrap_or("oot_aspmain_ucode");
            let base = args
                .get(4)
                .map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap())
                .unwrap_or(0x1080);
            print!("{}", emit_module(&words, base, fn_name));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("unknown mode {mode}");
            ExitCode::FAILURE
        }
    }
}
