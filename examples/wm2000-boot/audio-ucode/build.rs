//! Recompile the AKI-family audio microcode from the user's own ROM at build
//! time, into `OUT_DIR/aki_audio_ucode.rs`.
//!
//! Unlike the OoT adapter (which copies a pre-generated module a colleague
//! produced with a separate rsp-recomp step), this generates the module itself
//! via `fn64_audio::rsp::recomp::emit_module` — the AKI audio ucode needs no
//! new RSP ops (proven by `fn64-audio/tests/aki_ucode_coverage_probe.rs`), so
//! the emit is self-contained. No game bytes are stored in the repo: the ROM
//! is read from `ROM` (or `FN64_WM2000_ROM`) at build time and only the
//! generated Rust lands in `OUT_DIR`.
//!
//! The AKI audio ucode text is 3156 bytes at ROM offset 0x39510, vram
//! 0x80038910 — byte-identical across the AKI family (WT / Revenge / WM2000 /
//! No Mercy), located by exact unique full-length match in
//! `aki-recomp/games/NWXE/rsp/wm2000_audio.toml`.

use std::path::PathBuf;

/// AKI audio ucode text: ROM offset, byte length, and base vram (low bits).
const AKI_UCODE_ROM_OFF: usize = 0x39510;
const AKI_UCODE_LEN: usize = 0xC54; // 3156 bytes
const AKI_UCODE_BASE_VRAM: u32 = 0x0; // RSP ucode executes at IMEM 0x1000; the DRAM load addr 0x80038910 is NOT the IMEM base

fn main() {
    println!("cargo:rerun-if-env-changed=ROM");
    println!("cargo:rerun-if-env-changed=FN64_WM2000_ROM");

    let rom_path = std::env::var_os("ROM")
        .or_else(|| std::env::var_os("FN64_WM2000_ROM"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "aki-audio-ucode: no ROM. Set ROM (or FN64_WM2000_ROM) to your own \
                 legally-obtained WM2000 (.z64). The AKI audio ucode is recompiled from it \
                 at build time; nothing game-derived is stored in this repo."
            )
        });
    println!("cargo:rerun-if-changed={}", rom_path.display());

    let rom = std::fs::read(&rom_path)
        .unwrap_or_else(|e| panic!("aki-audio-ucode: reading {}: {e}", rom_path.display()));
    let end = AKI_UCODE_ROM_OFF + AKI_UCODE_LEN;
    assert!(
        rom.len() >= end,
        "aki-audio-ucode: ROM {} is only {} bytes; the AKI audio ucode text ends at 0x{end:x}. \
         Is this a WM2000 (or AKI-family) ROM?",
        rom_path.display(),
        rom.len()
    );
    let words: Vec<u32> = rom[AKI_UCODE_ROM_OFF..end]
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
        .collect();

    // Emit the recompiled module. The emitter writes a crate-level
    // `#![allow(...)]`; this adapter includes the file as a module (inner
    // attributes invalid there), so strip it — the adapter carries the same
    // allow at its own root.
    let source = fn64_audio::rsp::recomp::emit_module(&words, AKI_UCODE_BASE_VRAM, "aki_audio_ucode");
    let source = source
        .lines()
        .filter(|line| !line.starts_with("#![allow("))
        .collect::<Vec<_>>()
        .join("\n");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("aki_audio_ucode.rs");
    std::fs::write(&out, source)
        .unwrap_or_else(|e| panic!("aki-audio-ucode: writing {}: {e}", out.display()));
}
