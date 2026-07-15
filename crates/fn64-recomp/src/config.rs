//! Typed `RecompConfig`/`RspConfig` — our own representation of the
//! recompile inputs, per `docs/DECOUPLING.md`: "`RecompConfig` is our own
//! typed representation (not TOML strings) — sections, functions, stubs,
//! patches, hooks — so callers never hand-serialize N64Recomp's format."
//!
//! Field shapes are pulled directly from real N64Recomp configs already in
//! use for AKI titles (`aki-recomp/games/{NW4E,NWXE}/*.toml`,
//! `refs/WCWnWoRevengeRecomp/{revenge.toml,rsp/revenge_audio.toml}`) — this
//! is a faithful typed mirror of that observed `[input]`/`[patches]` shape
//! and the RSPRecomp microcode config shape, not a speculative redesign.

use std::path::PathBuf;

/// One `[[section]]` entry from the N64Recomp symbol TOML (`dump.toml`):
/// a contiguous rom/vram range plus the functions inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub rom: u32,
    pub vram: u32,
    pub size: u32,
    pub functions: Vec<Function>,
}

/// One function entry inside a `Section`'s `functions` list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub vram: u32,
    pub size: u32,
}

/// A single-instruction-word patch (`[[patches.instruction]]`): overwrite
/// one 32-bit word at `vram` inside `func`. Used for e.g. rewriting a
/// busy-spin branch into N64Recomp's recognized self-branch encoding (see
/// `wm2000.toml`'s idle-thread fix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionPatch {
    pub func: String,
    pub vram: u32,
    pub value: u32,
}

/// A source-level hook (`[[patches.hook]]`): splice raw recompiled-C `text`
/// immediately before the instruction at `before_vram` inside `func`,
/// without altering any MIPS bytes. Used for null-guarding a dispatch call
/// or short-circuiting a loop body (see `wm2000.toml`'s dispatch-table
/// fixes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hook {
    pub func: String,
    pub before_vram: u32,
    pub text: String,
}

/// Everything under `[patches]`: functions to stub out entirely, functions
/// to skip/ignore, and the instruction/hook patch lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patches {
    pub stubs: Vec<String>,
    pub ignored: Vec<String>,
    pub instructions: Vec<InstructionPatch>,
    pub hooks: Vec<Hook>,
}

/// Top-level input for `Recompiler::recompile` — a typed mirror of an
/// N64Recomp `[input]` + symbol-TOML + `[patches]` config. `sections` is
/// this config's own copy of the symbol table (what a `dump.toml` holds);
/// the adapter serializes it as a companion symbols file, matching
/// `symbols_file_path`'s real role (a separate TOML the `[input]` block
/// points at, not inlined).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecompConfig {
    pub entrypoint: u32,
    pub rom_file_path: PathBuf,
    /// Suffix N64Recomp uses to recognize a section's paired BSS section by
    /// name (e.g. `"_bss"`), per the real `[input]` key of the same name.
    pub bss_section_suffix: String,
    pub output_func_path: PathBuf,
    pub trace_mode: bool,
    pub sections: Vec<Section>,
    pub patches: Patches,
}

impl RecompConfig {
    pub fn new(entrypoint: u32, rom_file_path: impl Into<PathBuf>) -> Self {
        RecompConfig {
            entrypoint,
            rom_file_path: rom_file_path.into(),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("RecompiledFuncs"),
            trace_mode: false,
            sections: Vec::new(),
            patches: Patches::default(),
        }
    }
}

/// Input for `Recompiler::recompile_rsp` — a typed mirror of an RSPRecomp
/// microcode config (see `rsp/revenge_audio.toml`): locate one microcode's
/// text/data inside the ROM, plus any indirect (jump-table) branch targets
/// RSPRecomp can't discover by static scan alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspConfig {
    pub text_offset: u32,
    pub text_size: u32,
    pub text_address: u32,
    pub rom_file_path: PathBuf,
    pub output_function_name: String,
    pub extra_indirect_branch_targets: Vec<u32>,
}

impl RspConfig {
    pub fn new(
        text_offset: u32,
        text_size: u32,
        text_address: u32,
        rom_file_path: impl Into<PathBuf>,
        output_function_name: impl Into<String>,
    ) -> Self {
        RspConfig {
            text_offset,
            text_size,
            text_address,
            rom_file_path: rom_file_path.into(),
            output_function_name: output_function_name.into(),
            extra_indirect_branch_targets: Vec::new(),
        }
    }
}
