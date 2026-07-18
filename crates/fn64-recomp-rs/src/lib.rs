//! `fn64-recomp-rs`: a from-scratch, all-Rust static recompiler that turns
//! N64 VR4300 (MIPS III) machine code into **typed Rust** — the type-safe Rust
//! sibling of the N64Recomp shell-out adapter in `fn64-recomp`
//! (`docs/DECOUPLING.md` step 5, "Our implementation later:
//! `fn64-recomp-rs`").
//!
//! # Why
//!
//! The N64Recomp adapter emits **untyped C**: every memory access is a raw
//! pointer cast with a hand-written byte swizzle (`*(int16_t*)(rdram + ((reg +
//! off) ^ 2 - 0x…))`). That macro layer is the source of the byte-reinterpret
//! / offset / swizzle bug class this project has fought all session. This crate
//! makes that class *structurally impossible*: the emitted Rust never casts a
//! pointer, the swizzle lives in exactly one audited place
//! ([`runtime::Rdram`]), the whole crate (and the code it emits) is
//! `#![forbid(unsafe_code)]`, and every value carries its Rust type.
//!
//! # Scope of this foundation slice
//!
//! This is the *foundation*, not a whole-ROM recompiler: a complete
//! [`decoder`] for the core integer MIPS III ISA, a [`emit`]ter that produces
//! one typed Rust `fn` per MIPS function (delay slots, branch targets,
//! branch-likely, and the `LOOKUP_FUNC`-style indirect-call shape all handled
//! the way N64Recomp's `process_instruction` structures them), the typed
//! [`runtime`] those functions execute against, and a [`Recompiler`] impl so
//! it is a drop-in alternative to the adapter. It is validated against the
//! N64Recomp C oracle on a real MIPS function (see the crate tests).
//!
//! The byte-cited distinction between encoding coverage and full architectural
//! execution is maintained in `crates/fn64-recomp-rs/ISA-COVERAGE.md`.
//! Ordinary integer/control-flow/memory paths are covered; full COP1 floating
//! environment and privileged exception/MMU effects remain explicitly partial.
#![forbid(unsafe_code)]

pub mod decoder;
pub mod drive;
pub mod emit;
pub mod execution;
pub mod fallback;
pub mod interp;
pub mod module;
pub mod runtime;

pub use decoder::{decode, Instruction};
pub use drive::ExecutorAction;
pub use emit::{
    classify_bank_words, emit_bank_runner, emit_function, emit_function_resolved,
    emit_sparse_bank_runner, BankBlockInput, BankInput, BankWordCatalog, BankWordKind, BankWordRun,
    CallResolver, CallTarget, FuncInput, NullResolver, SparseBankInput,
};
pub use execution::{
    dispatch_until_boundary, BankError, BankId, BlockExit, BlockProgram, BlockRun, BlockRunner,
    CodeBank, CodeCatalog, CodeSpan, CpuFault, CpuFaultKind, DispatchError, DispatchRun,
    ExecutionKey, GeneratedBankFn, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError,
    ResolvedInstruction, TransferResolver,
};
pub use fallback::{EvidenceClass, FallbackProgram, FallbackRunner};
pub use interp::{run_bank, UnsupportedOp};
pub use module::{emit_lookup_dispatcher, emit_module, ModuleFunc, SymbolTable};
pub use runtime::{
    call_host_or_recompiled, pause_self, resolve_host_function, round_ties_even_f32,
    round_ties_even_f64, set_host_lookup, set_host_pause, HostLookup, HostPause, Rdram,
    RecompContext, RecompFunc, RDRAM_LEN, RDRAM_VBASE,
};

use fn64_recomp::{AbiVersion, RecompConfig, RecompError, RecompOutput, Recompiler, RspConfig};

/// The Rust recompiler. Implements the shared [`Recompiler`] trait so a
/// caller can A/B it against the N64Recomp adapter over identical
/// [`RecompConfig`]s (`docs/DECOUPLING.md`: "run both over identical input and
/// diff").
pub struct RsRecompiler {
    abi_version: AbiVersion,
}

impl RsRecompiler {
    /// Construct targeting a given fn64 ABI version (checked by the caller
    /// against `fn64-abi` before use, exactly like the adapter).
    pub fn new(abi_version: AbiVersion) -> Self {
        RsRecompiler { abi_version }
    }
}

impl Default for RsRecompiler {
    fn default() -> Self {
        RsRecompiler::new(AbiVersion::new(0, 0))
    }
}

impl Recompiler for RsRecompiler {
    fn recompile(&self, cfg: &RecompConfig) -> Result<RecompOutput, RecompError> {
        // Read the ROM once; each function's words are sliced out by its
        // section's rom offset + (vram - section vram).
        let rom = std::fs::read(&cfg.rom_file_path).map_err(|e| RecompError::Launch {
            binary: "fn64-recomp-rs".to_string(),
            reason: format!("could not read ROM {}: {e}", cfg.rom_file_path.display()),
        })?;

        let stubbed: std::collections::HashSet<&str> =
            cfg.patches.stubs.iter().map(String::as_str).collect();
        let ignored: std::collections::HashSet<&str> =
            cfg.patches.ignored.iter().map(String::as_str).collect();

        // The ELF/symbol front-end: build a vram -> name table over the WHOLE
        // config so every function's inter-function JAL/J calls resolve to a
        // direct Rust call to the named sibling `fn` (N64Recomp's `resolve_jal`
        // Match case), not a per-function `lookup()` stub. Stubbed/ignored
        // functions are excluded from the table so a call to one stays indirect.
        let symbols = module::SymbolTable::from_entries(cfg.sections.iter().flat_map(|s| {
            s.functions.iter().filter_map(|f| {
                if stubbed.contains(f.name.as_str()) || ignored.contains(f.name.as_str()) {
                    None
                } else {
                    Some((f.name.clone(), f.vram))
                }
            })
        }));

        let mut body = String::new();
        body.push_str("// Generated by fn64-recomp-rs. Typed Rust, no unsafe, no pointer casts.\n");
        body.push_str(
            "// Whole-program: inter-function JAL/J resolve to direct Rust calls where known.\n",
        );
        body.push_str("#![allow(clippy::all, unused, non_snake_case)]\n");
        body.push_str("#[allow(unused_imports)]\n");
        body.push_str(
            "use fn64_recomp_rs::{call_host_or_recompiled, pause_self, resolve_host_function, RecompContext, RecompFunc, Rdram, round_ties_even_f32, round_ties_even_f64};\n\n",
        );

        let mut recompiled = Vec::new();

        for section in &cfg.sections {
            for func in &section.functions {
                if stubbed.contains(func.name.as_str()) || ignored.contains(func.name.as_str()) {
                    continue;
                }
                let words = read_func_words(&rom, section, func).ok_or_else(|| {
                    RecompError::InvalidConfig(format!(
                        "function {} @ {:#010X} (size {}) is outside section {} rom range",
                        func.name, func.vram, func.size, section.name
                    ))
                })?;
                let input = FuncInput {
                    name: &func.name,
                    vram: func.vram,
                    words: &words,
                };
                body.push_str(&emit_function_resolved(&input, &symbols));
                body.push('\n');
                recompiled.push(func.name.clone());
            }
        }

        body.push_str(&module::emit_lookup_dispatcher(&symbols));

        let out_path = cfg.output_func_path.join("funcs.rs");
        Ok(RecompOutput {
            generated_files: vec![(out_path, body)],
            recompiled_functions: recompiled,
        })
    }

    fn recompile_rsp(&self, _cfg: &RspConfig) -> Result<RecompOutput, RecompError> {
        // The RSP microcode ISA is a different (scalar+vector) instruction set;
        // the concurrent fn64-audio RSP recompiler owns that path. This CPU
        // recompiler declines it loudly rather than emitting a wrong stub.
        Err(RecompError::InvalidConfig(
            "fn64-recomp-rs is a CPU (VR4300) recompiler; RSP microcode is out of scope \
             (see the fn64-audio RSP recompiler)"
                .to_string(),
        ))
    }

    fn abi_version(&self) -> AbiVersion {
        self.abi_version
    }
}

/// Slice a function's instruction words out of the ROM, converting each
/// big-endian 4-byte group to a host `u32`. Returns `None` if the function's
/// range falls outside the section's ROM extent.
fn read_func_words(
    rom: &[u8],
    section: &fn64_recomp::Section,
    func: &fn64_recomp::Function,
) -> Option<Vec<u32>> {
    // The function's ROM offset = section.rom + (func.vram - section.vram).
    let vram_delta = func.vram.checked_sub(section.vram)?;
    let start = (section.rom as usize).checked_add(vram_delta as usize)?;
    let len = (func.size as usize) & !0x3; // whole words only
    let end = start.checked_add(len)?;
    if end > rom.len() {
        return None;
    }
    let words = rom[start..end]
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Some(words)
}
