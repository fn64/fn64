//! Source-generation half of `fn64-recomp-rs`.
//!
//! Generated runners link only `fn64-recomp-rs`. Keeping this crate on the
//! build side of that boundary means an emitter-only edit cannot invalidate
//! the giant generated runner crates through their normal dependency graph.
#![forbid(unsafe_code)]

pub mod body_reuse;
pub mod emit;
pub mod module;
pub mod static_micro_op;

pub use body_reuse::{inventory_dense_body_reuse, DenseBodyReuseInventory};
pub use emit::{
    classify_bank_words, emit_bank_runner, emit_bank_runner_with_host_calls,
    emit_dense_bank_shard_runner_function, emit_dense_bank_shard_runner_function_with_host_calls,
    emit_function, emit_function_resolved, emit_sparse_bank_runner,
    emit_sparse_bank_runner_function, emit_sparse_bank_runner_function_with_host_calls,
    emit_sparse_bank_runner_with_host_calls, BankBlockInput, BankInput, BankWordCatalog,
    BankWordRun, CallResolver, CallTarget, DenseBankShardInput, DenseEmitError, FuncInput,
    NullResolver, SparseBankInput,
};
pub use fn64_recomp_rs::{BankId, BankWordKind};
pub use module::{emit_lookup_dispatcher, emit_module, ModuleFunc, SymbolTable};
pub use static_micro_op::{
    pack_static_micro_ops_v1, pack_static_micro_ops_v2, static_micro_op_packer_source_receipt_v1,
    static_micro_op_packer_source_receipt_v2, static_micro_op_packer_source_receipt_v3,
    validate_static_micro_op_pack_v1, validate_static_micro_op_pack_v2, StaticMicroOpPackError,
    StaticMicroOpPackV1, StaticMicroOpPackV2, StaticMicroOpPackerSourceReceiptV1,
    StaticMicroOpPackerSourceReceiptV2, StaticMicroOpPackerSourceReceiptV3, StaticMicroOpSpanInput,
    StaticMicroOpSpanInputV2, STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V1,
    STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V2, STATIC_MICRO_OP_PACKER_SOURCE_SCHEMA_V3,
    STATIC_MICRO_OP_PACK_SCHEMA_V1,
};

/// Whether one raw instruction owns an architectural delay slot.
///
/// Build-time shard tooling uses this instead of depending on the runtime
/// decoder directly, keeping the shared codegen crate as its single MIPS
/// classification boundary.
pub fn instruction_has_delay_slot(word: u32) -> bool {
    fn64_recomp_rs::decode(word).has_delay_slot()
}

use fn64_recomp::{AbiVersion, RecompConfig, RecompError, RecompOutput, Recompiler, RspConfig};
use sha2::{Digest, Sha256};

pub const GENERATED_RUNNER_EMITTER_SOURCE_SCHEMA_V2: &str =
    "fn64.generated-runner-emitter-source.v2";

/// Exact checked-in source identity of the arbitrary-PC Rust emitter.
///
/// This receipt describes source bytes only. The verifier-owned build remains
/// responsible for relating those bytes to a selected native callable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerEmitterSourceReceiptV2 {
    source_sha256: [u8; 32],
}

impl GeneratedRunnerEmitterSourceReceiptV2 {
    pub const fn schema(self) -> &'static str {
        GENERATED_RUNNER_EMITTER_SOURCE_SCHEMA_V2
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }
}

pub fn generated_runner_emitter_source_receipt_v2() -> GeneratedRunnerEmitterSourceReceiptV2 {
    let sources: &[(&[u8], &[u8])] = &[
        (b"Cargo.toml", include_bytes!("../Cargo.toml")),
        (b"src/lib.rs", include_bytes!("lib.rs")),
        // `body_reuse.rs` emits content-silent observations only. It cannot
        // change generated runner bytes and therefore is not emitter source.
        (b"src/emit.rs", include_bytes!("emit.rs")),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:generated-runner-emitter-source:v2:");
    for (label, source) in sources {
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label);
        hasher.update((source.len() as u64).to_be_bytes());
        hasher.update(source);
    }
    GeneratedRunnerEmitterSourceReceiptV2 {
        source_sha256: hasher.finalize().into(),
    }
}

/// Whole-program typed-Rust recompiler built on the linked runtime crate.
pub struct RsRecompiler {
    abi_version: AbiVersion,
}

impl RsRecompiler {
    pub fn new(abi_version: AbiVersion) -> Self {
        Self { abi_version }
    }
}

impl Default for RsRecompiler {
    fn default() -> Self {
        Self::new(AbiVersion::new(0, 0))
    }
}

impl Recompiler for RsRecompiler {
    fn recompile(&self, cfg: &RecompConfig) -> Result<RecompOutput, RecompError> {
        let rom = std::fs::read(&cfg.rom_file_path).map_err(|error| RecompError::Launch {
            binary: "fn64-recomp-rs".to_string(),
            reason: format!(
                "could not read ROM {}: {error}",
                cfg.rom_file_path.display()
            ),
        })?;
        let stubbed = cfg
            .patches
            .stubs
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        let ignored = cfg
            .patches
            .ignored
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        let symbols = SymbolTable::from_entries(cfg.sections.iter().flat_map(|section| {
            section.functions.iter().filter_map(|function| {
                (!stubbed.contains(function.name.as_str())
                    && !ignored.contains(function.name.as_str()))
                .then(|| (function.name.clone(), function.vram))
            })
        }));
        let mut body = String::new();
        body.push_str("// Generated by fn64-recomp-rs. Typed Rust, no unsafe, no pointer casts.\n");
        body.push_str(
            "// Whole-program: inter-function JAL/J resolve to direct Rust calls where known.\n",
        );
        body.push_str("#![allow(clippy::all, unused, non_snake_case)]\n");
        body.push_str("pub const FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA: fn64_recomp_rs::FunctionEntryObservationSchema = fn64_recomp_rs::FUNCTION_ENTRY_OBSERVATION_SCHEMA;\n");
        body.push_str("#[allow(unused_imports)]\n");
        body.push_str("use fn64_recomp_rs::{call_host_or_recompiled, pause_self, resolve_host_function, RecompContext, RecompFunc, Rdram, round_ties_even_f32, round_ties_even_f64};\n\n");
        let mut recompiled = Vec::new();
        for section in &cfg.sections {
            for function in &section.functions {
                if stubbed.contains(function.name.as_str())
                    || ignored.contains(function.name.as_str())
                {
                    continue;
                }
                let words = read_func_words(&rom, section, function).ok_or_else(|| {
                    RecompError::InvalidConfig(format!(
                        "function {} @ {:#010X} (size {}) is outside section {} rom range",
                        function.name, function.vram, function.size, section.name
                    ))
                })?;
                body.push_str(&emit_function_resolved(
                    &FuncInput {
                        name: &function.name,
                        vram: function.vram,
                        words: &words,
                    },
                    &symbols,
                ));
                body.push('\n');
                recompiled.push(function.name.clone());
            }
        }
        body.push_str(&emit_lookup_dispatcher(&symbols));
        Ok(RecompOutput {
            generated_files: vec![(cfg.output_func_path.join("funcs.rs"), body)],
            recompiled_functions: recompiled,
        })
    }

    fn recompile_rsp(&self, _cfg: &RspConfig) -> Result<RecompOutput, RecompError> {
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

fn read_func_words(
    rom: &[u8],
    section: &fn64_recomp::Section,
    function: &fn64_recomp::Function,
) -> Option<Vec<u32>> {
    let vram_delta = function.vram.checked_sub(section.vram)?;
    let start = (section.rom as usize).checked_add(vram_delta as usize)?;
    let len = (function.size as usize) & !0x3;
    let end = start.checked_add(len)?;
    (end <= rom.len()).then(|| {
        rom[start..end]
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::instruction_has_delay_slot;

    #[test]
    fn shared_delay_slot_query_uses_the_runtime_decoder() {
        assert!(instruction_has_delay_slot(0x03e0_0008)); // jr $ra
        assert!(instruction_has_delay_slot(0x1000_0001)); // beq
        assert!(!instruction_has_delay_slot(0x2402_0001)); // addiu
    }
}
