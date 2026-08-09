//! Shared pure source generation for the legacy shard build and one-shot producer.
//!
//! Returned strings contain ROM-derived game content. Callers must keep them
//! inside Cargo `OUT_DIR` or the private prepared-tree publication boundary.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use fn64_discover::delta_vote::DeltaVoteConfig;
use fn64_discover::overlay_recipe::{admitted_overlay_load_recipes_v1, OverlayLoadRecipeV1};
use fn64_discover::overlay_regions::{recover_overlay_regions, SearchConfig};
use sha2::{Digest, Sha256};

// `ROM_START` and `BOOT_BYTES` are fixed by N64 hardware boot behavior: every
// standard IPL3 DMAs exactly 1 MiB from ROM 0x1000. They are the same two
// constants discovery publishes as `banks::BOOT_COPY_ROM_START` and
// `banks::BOOT_COPY_SIZE`, and `boot_bank_va_start` asserts the agreement.
//
// The boot copy's *virtual* base is NOT universal -- it is the header entry
// point minus a CIC-dependent load delta (0 for 6102/6105/7102, 0x100000 for
// 6103, 0x200000 for 6106; see `crates/fn64-discover/src/banks/mod.rs`). It is
// derived per ROM by `boot_bank_va_start`, never assumed.
const ROM_START: usize = 0x1000;
const BOOT_BYTES: usize = 0x10_0000;
const SHARD_BYTES: usize = 64 * 1024;
// The artifact boundary remains 64 KiB, while static subrunners keep rustc
// below the measured memory ceiling. Transfers leave through BlockProgram.
const RUNNER_BYTES: usize = 2 * 1024;

/// The one shard inventory, shared verbatim with the prepared materializer,
/// the WM root pack build and the verifier. See `shard_inventory.in`.
pub const SHARD_INVENTORY: &[(&str, &str)] = &include!("shard_inventory.in");
pub const SHARD_COUNT: usize = SHARD_INVENTORY.len();
pub const PACKAGES: [&str; SHARD_COUNT] = {
    let mut packages = [""; SHARD_COUNT];
    let mut index = 0;
    while index < SHARD_COUNT {
        packages[index] = SHARD_INVENTORY[index].0;
        index += 1;
    }
    packages
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedShard {
    pub package: String,
    pub runner: String,
    pub metadata: String,
    pub reuse_2k: fn64_recomp_rs_codegen::DenseBodyReuseInventory,
    pub reuse_64k: fn64_recomp_rs_codegen::DenseBodyReuseInventory,
    pub static_micro_op_bytes: usize,
    pub static_micro_op_instructions: u64,
    pub static_micro_op_body_sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpShardProfile {
    pub bytes: usize,
    pub instructions: u64,
    pub body_sha256: [u8; 32],
}

#[derive(Clone, Debug)]
struct Generation {
    name: String,
    source_start: usize,
    source_end: usize,
    /// End of the source bytes that remain affine with consecutive guest VAs.
    /// Ownership may end earlier; one following word is usable only as a
    /// live-verified architectural delay instruction, never as owned code.
    affine_source_end: usize,
    va_start: u32,
}

struct ResolvedShard {
    generation: Generation,
    index: usize,
    image_byte_len: usize,
    byte_start: usize,
    byte_end: usize,
    va_start: u32,
    byte_len: usize,
    words: Vec<u32>,
    /// Affine next word used only when the final owned word is a control.
    /// This is not part of the shard's owned instruction identity.
    delay_lookahead: Option<u32>,
    id: u64,
}

impl ResolvedShard {
    fn static_micro_op_profile(&self) -> StaticMicroOpShardProfile {
        let pack = fn64_recomp_rs_codegen::pack_static_micro_ops_v2(&[
            fn64_recomp_rs_codegen::StaticMicroOpSpanInputV2 {
                bank: fn64_recomp_rs_codegen::BankId::new(self.id),
                vram: self.va_start,
                words: &self.words,
                delay_lookahead: self.delay_lookahead,
            },
        ])
        .expect("packing canonical static micro-ops for generated shard");
        StaticMicroOpShardProfile {
            bytes: pack.bytes().len(),
            instructions: pack.instruction_count(),
            body_sha256: pack.body_sha256(),
        }
    }
}

/// Whether emitted runners re-read each instruction word from live guest
/// memory before executing it (`verify_live_words`).
///
/// **This flag controls a DETECTOR, not redundant work.** Read the whole note
/// before turning it off, and see `docs/plans/perf-method.md`.
///
/// # What it costs
///
/// The check is emitted at the top of the runner's `'run: loop`, above the
/// `match pc`, so it executes **once per guest instruction**: a bounds test, an
/// `EXPECTED_WORDS` index, and a full `Rdram::load_w` guest load. Ablated
/// against the real emitter output on a realistic WM2000 instruction mix it
/// measures **3.10 ns/instruction** — the single largest per-instruction cost
/// in the emitted body, ahead of `advance_cop0_random` (0.61 ns) and
/// `post_straight_instruction_exit` (0.26 ns).
///
/// # What it detects, and what still detects it when this is off
///
/// It catches a guest write that changes executable bytes underneath a live
/// translation. Two other mechanisms cover that, and they are what the
/// opt-out rests on:
///
/// - **Declared writes** reach `request_guest_write_boundary`
///   (`fn64-recomp-rs` `runtime/host.rs:531`) → `classify_live_executable_write`
///   (`fn64-abi` `recompiled/snapshots.rs:1289`), which sets
///   `EXECUTABLE_WRITE_BOUNDARY` when the write hits a resident executable
///   range. `post_straight_instruction_exit` consumes that at every
///   architectural instruction boundary and exits with
///   `BlockExit::ExecutableWrite`.
/// - **The un-resident case** is covered by `activate_for_fetch_with_digest`
///   (`fn64-recomp-rs` `generation/mod.rs`), which re-digests LIVE memory
///   before activating any generation, so bytes changed while nothing was
///   resident cannot later be executed as stale code.
///
/// # The gap this opt-out accepts
///
/// `fn64-abi` `write_barrier.rs:52-57` lists writers that bypass the
/// declaration channel — `as_mut_slice`, the DMA paths, the RSP/renderer
/// slices and raw `RdramPtr` stores. `verify_live_words` was the belt-and-
/// braces detector for exactly those. Turning it off is a **defence-in-depth
/// removal**, justified on a given route only by the seven-counter byte
/// identity holding across the A/B, never by the two mechanisms above alone.
///
/// # Default
///
/// **On**, matching the behaviour that predates this flag. Set
/// `FN64_WM_SHARD_VERIFY_LIVE_WORDS=0` to opt out. Absent, empty and `0` all
/// mean the same thing on the *other* flags in this tree (`fn64-abi`'s
/// `env_flag`), and an empty value reading as "set" is precisely what
/// fabricated a 4.9x speedup once before — so this parses explicitly and
/// treats anything it does not recognise as "leave verification on".
fn emit_live_word_verification() -> bool {
    println!("cargo:rerun-if-env-changed=FN64_WM_SHARD_VERIFY_LIVE_WORDS");
    match std::env::var_os("FN64_WM_SHARD_VERIFY_LIVE_WORDS") {
        None => true,
        Some(value) => !matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
    }
}

fn delay_lookahead_word(
    rom: &[u8],
    generation: &Generation,
    next_word_index: usize,
) -> Option<u32> {
    let start = generation
        .source_start
        .checked_add(next_word_index.checked_mul(4)?)?;
    let end = start.checked_add(4)?;
    if end > generation.affine_source_end {
        return None;
    }
    rom.get(start..end)
        .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
}

#[derive(Clone, Copy, Debug)]
enum PackageTarget {
    Boot(usize),
    ResidentTail(usize),
    Overlay { generation: usize, shard: usize },
}

/// Split the 1 MiB boot copy into its `(boot, resident_tail)` shard counts.
///
/// This is the whole resident topology rule, and it is title-generic: the two
/// runs tile `[va_start, split)` and `[split, va_start + BOOT_BYTES)` at 64 KiB
/// with a possibly-partial final shard each, and together they cover the boot
/// copy exactly once. `split` is a ROM offset inside the boot copy.
fn resident_shard_counts(split: usize) -> (usize, usize) {
    assert!(
        ROM_START < split && split < ROM_START + BOOT_BYTES,
        "resident split {split:#x} must lie strictly inside the boot copy"
    );
    (
        (split - ROM_START).div_ceil(SHARD_BYTES),
        (ROM_START + BOOT_BYTES - split).div_ceil(SHARD_BYTES),
    )
}

/// The boot copy's proven virtual base, taken from the discovered boot-bank
/// `RomMapping` rather than assumed. The mapping's ROM extent must be exactly
/// the IPL3 boot DMA this generator tiles, so the two agree by construction.
fn boot_bank_va_start(rom: &fn64_discover::rom::NormalizedRom) -> u32 {
    let mut db = fn64_discover::FactDb::new();
    let discovery = fn64_discover::banks::discover_boot_bank(rom, &mut db);
    assert!(
        matches!(
            discovery,
            fn64_discover::banks::BootBankDiscovery::Proven { .. }
        ),
        "dense-AOT shard generation requires a proven IPL3-bound boot bank, got {discovery:?}"
    );
    let (rom_start, rom_end, va_start) = db
        .proven_rom_mappings()
        .into_iter()
        .find_map(|fact| match fact {
            fn64_discover::Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if bank == fn64_discover::banks::BOOT_BANK => Some((*rom_start, *rom_end, *va_start)),
            _ => None,
        })
        .expect("proven boot bank publishes its ROM mapping");
    assert_eq!(
        (rom_start as usize, rom_end as usize),
        (ROM_START, ROM_START + BOOT_BYTES),
        "boot bank ROM extent must be the fixed IPL3 boot DMA this generator tiles"
    );
    va_start
}

pub struct WmShardGenerator {
    rom: fn64_discover::rom::NormalizedRom,
    overlay_recipes: Option<Vec<OverlayLoadRecipeV1>>,
    host_calls: Vec<u32>,
    /// Proven boot-copy virtual base. CIC-dependent, so never a literal.
    boot_va_start: u32,
}

impl WmShardGenerator {
    pub fn from_rom_bytes(source: &[u8]) -> Self {
        let rom = fn64_discover::normalize(source).expect("normalizing shard ROM input");
        let boot_va_start = boot_bank_va_start(&rom);
        let resident_signature = rom.bytes[ROM_START..ROM_START + BOOT_BYTES]
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        let host_bindings = fn64_discover::host_bindings::discover_wm_block_runtime_host_bindings(
            &resident_signature,
            boot_va_start,
        )
        .expect("discovering exact WM block runtime host bindings");
        assert_eq!(
            host_bindings.len(),
            fn64_discover::host_bindings::WM_BLOCK_RUNTIME_HOST_SYMBOLS.len(),
            "shared WM host catalog must remain exact"
        );
        Self {
            rom,
            overlay_recipes: None,
            host_calls: host_bindings.iter().map(|binding| binding.vram).collect(),
            boot_va_start,
        }
    }

    /// Construct only enough state to derive package topology.
    ///
    /// Topology depends on the boot mapping and recovered overlay recipes, not
    /// on whether every runtime host adapter has a recognizer for this title's
    /// libultra revision. Runner emission continues to use `from_rom_bytes`
    /// and therefore keeps the exact 15/15 host-binding gate loud.
    pub fn from_rom_bytes_for_topology(source: &[u8]) -> Self {
        let rom = fn64_discover::normalize(source).expect("normalizing shard ROM input");
        let boot_va_start = boot_bank_va_start(&rom);
        Self {
            rom,
            overlay_recipes: None,
            host_calls: Vec::new(),
            boot_va_start,
        }
    }

    pub fn normalized_rom_sha256(&self) -> [u8; 32] {
        let source = self.rom.sha256.as_bytes();
        assert_eq!(
            source.len(),
            64,
            "normalized ROM digest is canonical SHA-256"
        );
        let mut digest = [0u8; 32];
        for (index, pair) in source.chunks_exact(2).enumerate() {
            digest[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
        }
        digest
    }

    /// Derive the complete Cargo package topology for this ROM.
    ///
    /// Package names remain the stable harness ABI; only their count and
    /// manifest-directory mapping vary by title. Resident manifests retain
    /// the historical contiguous `shardNN` directory scheme so a generated
    /// topology round-trips the committed WM2000 tree byte-for-byte.
    pub fn package_inventory(&mut self) -> Vec<(String, String)> {
        let split = self.resident_split();
        let (boot_count, tail_count) = resident_shard_counts(split);
        let mut inventory = Vec::new();
        for index in 0..boot_count {
            inventory.push((
                format!("wm2000-block-shard-{index:02}"),
                format!("shard{index:02}"),
            ));
        }
        for index in 0..tail_count {
            inventory.push((
                format!("wm2000-block-resident-tail-shard-{index:02}"),
                format!("shard{:02}", boot_count + index),
            ));
        }
        let overlay_counts = self
            .overlay_recipes()
            .iter()
            .map(|recipe| {
                usize::try_from(fn64_discover::overlay_recipe::generation_source_span(recipe))
                    .expect("overlay source span fits usize")
                    .div_ceil(SHARD_BYTES)
            })
            .collect::<Vec<_>>();
        for (generation, shard_count) in overlay_counts.into_iter().enumerate() {
            for shard in 0..shard_count {
                inventory.push((
                    format!("wm2000-block-overlay-{generation}-shard-{shard:02}"),
                    format!("overlay{generation}-shard{shard:02}"),
                ));
            }
        }
        inventory.sort();
        inventory
    }

    pub fn generate_package(&mut self, package: &str) -> GeneratedShard {
        let resolved = self.resolve_shard(package);
        let generation = &resolved.generation;
        let index = resolved.index;
        let image_byte_len = resolved.image_byte_len;
        let byte_start = resolved.byte_start;
        let byte_end = resolved.byte_end;
        let va_start = resolved.va_start;
        let byte_len = resolved.byte_len;
        let words = &resolved.words;
        let id = resolved.id;
        let bytes = &self.rom.bytes[byte_start..byte_end];
        let mut runner = String::new();
        for (runner_index, runner_words) in words.chunks(RUNNER_BYTES / 4).enumerate() {
            let runner_va = va_start + u32::try_from(runner_index * RUNNER_BYTES).unwrap();
            let runner_name = format!("run_{runner_index:02}");
            let image_word_index = index * (SHARD_BYTES / 4) + runner_index * (RUNNER_BYTES / 4);
            let delay_lookahead = delay_lookahead_word(
                &self.rom.bytes,
                &generation,
                image_word_index + runner_words.len(),
            );
            writeln!(runner, "mod runner_{runner_index:02} {{").unwrap();
            writeln!(runner, "use super::*;").unwrap();
            runner.push_str(
                &fn64_recomp_rs_codegen::emit_dense_bank_shard_runner_function_with_host_calls(
                    &fn64_recomp_rs_codegen::DenseBankShardInput {
                        name: &runner_name,
                        bank: fn64_recomp_rs_codegen::BankId::new(id),
                        image_vram_start: generation.va_start,
                        image_vram_end: generation.va_start
                            + u32::try_from(image_byte_len).unwrap(),
                        artifact_vram_start: va_start,
                        artifact_vram_end: va_start + u32::try_from(byte_len).unwrap(),
                        shard_vram_start: runner_va,
                        words: runner_words,
                        delay_lookahead,
                        verify_live_words: emit_live_word_verification(),
                    },
                    &self.host_calls,
                )
                .unwrap_or_else(|error| {
                    panic!("dense runner {runner_name} at {runner_va:#010x} is invalid: {error:?}")
                }),
            );
            writeln!(runner, "}}").unwrap();
        }
        writeln!(
            runner,
            "pub fn run(entry: ExecutionKey, budget: InstructionBudget, ctx: &mut RecompContext, mem: &mut Rdram) -> BlockRun {{"
        )
        .unwrap();
        writeln!(runner, "    match entry.pc.get() {{").unwrap();
        for (runner_index, runner_words) in words.chunks(RUNNER_BYTES / 4).enumerate() {
            let runner_va = va_start + u32::try_from(runner_index * RUNNER_BYTES).unwrap();
            let runner_byte_len = runner_words.len() * 4;
            let runner_end = runner_va + u32::try_from(runner_byte_len).unwrap() - 1;
            writeln!(
                runner,
                "        {runner_va:#010X}..={runner_end:#010X} => runner_{runner_index:02}::run_{runner_index:02}(entry, budget, ctx, mem),"
            )
            .unwrap();
        }
        writeln!(
            runner,
            "        _ => BlockRun::new(BlockExit::Fault(CpuFault {{ at: entry, kind: CpuFaultKind::UnmappedPc {{ bank_start: {va_start:#010X}, bank_end: {:#010X} }} }}), 0),",
            va_start + u32::try_from(byte_len).unwrap(),
        )
        .unwrap();
        writeln!(runner, "    }}\n}}").unwrap();

        let source_sha256 = fn64_discover::dense_aot_pack::dense_aot_shard_source_identity(
            &self.rom.sha256,
            &generation.name,
            u32::try_from(byte_start).unwrap(),
            u32::try_from(byte_end).unwrap(),
            va_start,
            va_start + u32::try_from(byte_len).unwrap(),
            bytes,
        );
        let static_micro_ops = resolved.static_micro_op_profile();
        let runner_source_sha256: [u8; 32] = Sha256::digest(runner.as_bytes()).into();
        let mut metadata = String::new();
        let _ = writeln!(metadata, "pub const BANK_ID: u64 = {id:#018X};");
        let _ = writeln!(metadata, "pub const VA_START: u32 = {va_start:#010X};");
        let _ = writeln!(metadata, "pub const BYTE_LEN: u32 = {byte_len:#010X};");
        // Geometry, not content: where these instruction words live in the
        // user's normalized ROM. `code_bank()` reads them from the runtime ROM
        // and the existing `code_bank_sha256` assertion in `block_program.rs`
        // proves the recovered words are the ones this shard was built from.
        // These offsets index the NORMALIZED big-endian image (`self.rom` is
        // `fn64_discover::normalize(source)`), so the runtime normalizes before
        // slicing and a .n64/.v64 user file resolves to one identity.
        let _ = writeln!(
            metadata,
            "pub const ROM_START: u32 = {:#010X};",
            u32::try_from(byte_start).unwrap()
        );
        let _ = writeln!(
            metadata,
            "pub const ROM_END: u32 = {:#010X};",
            u32::try_from(byte_end).unwrap()
        );
        let _ = writeln!(
            metadata,
            "pub const SOURCE_SHA256: [u8; 32] = {source_sha256:?};"
        );
        let _ = writeln!(
            metadata,
            "pub const RUNNER_SOURCE_SHA256: [u8; 32] = {runner_source_sha256:?};"
        );
        GeneratedShard {
            package: package.to_owned(),
            runner,
            metadata,
            reuse_2k: fn64_recomp_rs_codegen::inventory_dense_body_reuse(words, RUNNER_BYTES / 4),
            reuse_64k: fn64_recomp_rs_codegen::inventory_dense_body_reuse(words, SHARD_BYTES / 4),
            static_micro_op_bytes: static_micro_ops.bytes,
            static_micro_op_instructions: static_micro_ops.instructions,
            static_micro_op_body_sha256: static_micro_ops.body_sha256,
        }
    }

    /// Measure one canonical packed shard without constructing generated Rust.
    pub fn profile_static_micro_ops(&mut self, package: &str) -> StaticMicroOpShardProfile {
        self.resolve_shard(package).static_micro_op_profile()
    }

    fn resolve_shard(&mut self, package: &str) -> ResolvedShard {
        let target = package_target(package);
        let (generation, index) = self.resolve_generation(target);
        let image_byte_len = generation.source_end - generation.source_start;
        let shard_count = image_byte_len.div_ceil(SHARD_BYTES);
        assert!(
            index < shard_count,
            "shard index {index} is outside generation {} with {shard_count} shards",
            generation.name
        );
        let byte_start = generation.source_start + index * SHARD_BYTES;
        let byte_end = (byte_start + SHARD_BYTES).min(generation.source_end);
        let bytes = self
            .rom
            .bytes
            .get(byte_start..byte_end)
            .expect("ROM contains recovered generation shard");
        let words = bytes
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        let va_start = generation.va_start + u32::try_from(index * SHARD_BYTES).unwrap();
        let id = fn64_discover::dense_aot_pack::dense_aot_artifact_bank_id(
            &self.rom.sha256,
            &generation.name,
            va_start,
            &words,
        );
        let delay_lookahead = words
            .last()
            .copied()
            .filter(|word| fn64_recomp_rs_codegen::instruction_has_delay_slot(*word))
            .and_then(|_| {
                delay_lookahead_word(
                    &self.rom.bytes,
                    &generation,
                    index * (SHARD_BYTES / 4) + words.len(),
                )
            });
        ResolvedShard {
            generation,
            index,
            image_byte_len,
            byte_start,
            byte_end,
            va_start,
            byte_len: bytes.len(),
            words,
            delay_lookahead,
            id,
        }
    }

    fn resolve_generation(&mut self, target: PackageTarget) -> (Generation, usize) {
        match target {
            // Boot and resident tail are the two halves of the one 1 MiB
            // affine boot copy, split at the first overlay's load address:
            // boot tiles `[va_start, first_overlay_start)` and the tail tiles
            // `[first_overlay_start, va_start + BOOT_BYTES)`. Both runs are a
            // plain 64 KiB tiling whose last shard may be partial, so neither
            // needs to know which index the split lands on. `resolve_shard`
            // bounds each index against the run's own derived shard count.
            //
            // Both keep `affine_source_end` at the end of the whole boot copy:
            // the ROM bytes stay affine with consecutive guest VAs across the
            // split, so the final owned word of either run may take its
            // architectural delay instruction from the following word.
            PackageTarget::Boot(index) => {
                let split = self.resident_split();
                (
                    Generation {
                        name: "boot".to_string(),
                        source_start: ROM_START,
                        source_end: split,
                        affine_source_end: ROM_START + BOOT_BYTES,
                        va_start: self.boot_va_start,
                    },
                    index,
                )
            }
            PackageTarget::ResidentTail(index) => {
                let split = self.resident_split();
                (
                    Generation {
                        name: "resident_tail".to_string(),
                        source_start: split,
                        source_end: ROM_START + BOOT_BYTES,
                        affine_source_end: ROM_START + BOOT_BYTES,
                        va_start: self.first_overlay_start(),
                    },
                    index,
                )
            }
            PackageTarget::Overlay {
                generation: overlay_index,
                shard: index,
            } => {
                let recipe = self
                    .overlay_recipes()
                    .get(overlay_index)
                    .unwrap_or_else(|| panic!("overlay generation {overlay_index} is absent"));
                // Admit only the TEXT extent as executable image. `rom_end`
                // covers the overlay's data section too, and a correct program
                // writes its own data at runtime -- WM2000 stores four bytes at
                // VA 0x80107efc inside overlay 0's data span, which
                // invalidated the whole generation and stopped the route.
                //
                // Derived from the one shared helper so these shard extents
                // match the ones the pack emits -- both are folded into
                // catalog digests that have to agree.
                let source_end = (recipe.rom_start
                    + fn64_discover::overlay_recipe::generation_source_span(recipe))
                    as usize;
                (
                    Generation {
                        name: format!("recovered_overlay_{overlay_index}"),
                        source_start: recipe.rom_start as usize,
                        source_end,
                        affine_source_end: source_end,
                        va_start: recipe.load_start,
                    },
                    index,
                )
            }
        }
    }

    /// ROM offset where the resident boot copy stops being static prefix and
    /// starts being overlay-invalidated tail.
    ///
    /// The invariant that survives from the retired WM2000-shaped assertions
    /// is the one that is genuinely general: the first overlay load must land
    /// strictly inside the boot copy, so both runs are nonempty. Which shard
    /// index the split falls in is per-title (WM2000: 14, No Mercy: 13) and is
    /// deliberately no longer asserted.
    fn resident_split(&mut self) -> usize {
        let va_start = self.boot_va_start;
        let first_overlay_start = self.first_overlay_start();
        assert!(
            va_start < first_overlay_start,
            "first overlay load {first_overlay_start:#010X} must lie above the boot base \
             {va_start:#010X}"
        );
        // Both runs are decoded as whole instruction words, so the split must
        // fall on a word boundary or a shard would silently drop a tail byte.
        assert!(
            first_overlay_start.is_multiple_of(4),
            "first overlay load {first_overlay_start:#010X} must be instruction-aligned"
        );
        let split = ROM_START
            + usize::try_from(first_overlay_start - va_start).expect("static prefix fits usize");
        // The one topology rule both rejects a split outside the boot copy --
        // the first overlay must land strictly inside it, leaving both runs
        // nonempty -- and states the tiling `resolve_shard` then applies to
        // each run's generation extent.
        resident_shard_counts(split);
        split
    }

    fn first_overlay_start(&mut self) -> u32 {
        self.overlay_recipes()
            .iter()
            .map(|recipe| recipe.load_start)
            .min()
            .expect("recovered overlay catalog is nonempty")
    }

    fn overlay_recipes(&mut self) -> &[OverlayLoadRecipeV1] {
        self.overlay_recipes.get_or_insert_with(|| {
            let search = SearchConfig::aki_family();
            let recovery = recover_overlay_regions(
                &self.rom.bytes,
                &search,
                &DeltaVoteConfig::default(),
                search.min_records,
            );
            admitted_overlay_load_recipes_v1(&self.rom.bytes, &recovery)
                .expect("recovering one unambiguous complete overlay recipe table")
        })
    }
}

fn package_target(package: &str) -> PackageTarget {
    if let Some(index) = package.strip_prefix("wm2000-block-shard-") {
        return PackageTarget::Boot(index.parse().expect("boot shard package suffix is decimal"));
    }
    if let Some(index) = package.strip_prefix("wm2000-block-resident-tail-shard-") {
        return PackageTarget::ResidentTail(
            index
                .parse()
                .expect("resident-tail shard package suffix is decimal"),
        );
    }
    let suffix = package
        .strip_prefix("wm2000-block-overlay-")
        .expect("package names either a boot shard or overlay shard");
    let (generation, shard) = suffix
        .split_once("-shard-")
        .expect("overlay package name contains -shard-");
    PackageTarget::Overlay {
        generation: generation
            .parse()
            .expect("overlay generation suffix is decimal"),
        shard: shard.parse().expect("overlay shard suffix is decimal"),
    }
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => panic!("normalized ROM digest is lowercase hexadecimal"),
    }
}

fn write_if_changed(path: &Path, contents: &str) {
    if std::fs::read(path).is_ok_and(|existing| existing == contents.as_bytes()) {
        return;
    }
    std::fs::write(path, contents).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_lookahead_crosses_only_an_affine_ownership_boundary() {
        let words = [0x1111_1111u32, 0x2222_2222, 0x3333_3333];
        let rom = words
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();
        let generation = Generation {
            name: "split".to_owned(),
            source_start: 0,
            source_end: 8,
            affine_source_end: 12,
            va_start: 0x8000_0000,
        };
        assert_eq!(delay_lookahead_word(&rom, &generation, 2), Some(words[2]));

        let non_affine = Generation {
            affine_source_end: generation.source_end,
            ..generation
        };
        assert_eq!(delay_lookahead_word(&rom, &non_affine, 2), None);
    }

    /// The resident topology is a tiling rule, not a per-title constant. Both
    /// measured AKI splits are exercised, and neither is privileged: WM2000
    /// puts the split in boot shard 14, No Mercy in boot shard 13.
    #[test]
    fn resident_runs_tile_the_boot_copy_for_every_split() {
        // WM2000: first overlay at VA 0x800E1B90 over a 0x80000400 base.
        assert_eq!(resident_shard_counts(ROM_START + 0xe_1790), (15, 2));
        // No Mercy: first overlay at VA 0x800D9960 over the same base. Under
        // the retired `Boot(0..=13)` / `== 2` constants this was rejected.
        assert_eq!(resident_shard_counts(ROM_START + 0xd_9560), (14, 3));

        // Exhaustive over every word-aligned split: expanding both runs into
        // their 64 KiB shard extents must reproduce the boot copy exactly --
        // contiguous from `ROM_START`, no gap, no overlap, no byte past the
        // end -- with only each run's final shard allowed to be partial.
        for split_offset in (4..BOOT_BYTES).step_by(4) {
            let split = ROM_START + split_offset;
            let (boot, tail) = resident_shard_counts(split);
            assert!(boot > 0 && tail > 0, "both runs nonempty at {split:#x}");

            let mut cursor = ROM_START;
            for (run_start, run_end, count) in [
                (ROM_START, split, boot),
                (split, ROM_START + BOOT_BYTES, tail),
            ] {
                for index in 0..count {
                    let start = run_start + index * SHARD_BYTES;
                    let end = (start + SHARD_BYTES).min(run_end);
                    assert_eq!(start, cursor, "shard tiling has a gap at {split:#x}");
                    assert!(start < end, "empty shard at {split:#x}");
                    assert!(
                        end - start == SHARD_BYTES || index + 1 == count,
                        "only a run's final shard may be partial, at {split:#x}"
                    );
                    cursor = end;
                }
                assert_eq!(cursor, run_end, "run does not reach its end at {split:#x}");
            }
            assert_eq!(
                cursor,
                ROM_START + BOOT_BYTES,
                "runs do not cover the boot copy at {split:#x}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "must lie strictly inside the boot copy")]
    fn a_split_outside_the_boot_copy_is_rejected() {
        resident_shard_counts(ROM_START + BOOT_BYTES);
    }
}

// Cargo executes this function only when this measured file is the legacy
// build script. The producer imports the same file as a module, where this is
// an ordinary inert module function.
fn main() {
    println!("cargo:rerun-if-env-changed=ROM");
    let profile = std::env::var_os("FN64_PROFILE_BUILD").is_some();
    let started = Instant::now();
    let package = std::env::var("CARGO_PKG_NAME").expect("Cargo supplies package name");
    let rom_path = std::env::var("ROM").expect("ROM must name the user's NWXE image");
    println!("cargo:rerun-if-changed={rom_path}");
    let source = std::fs::read(&rom_path).expect("reading shard ROM input");
    let mut source_generator = WmShardGenerator::from_rom_bytes(&source);
    let generated = source_generator.generate_package(&package);
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    write_if_changed(&out.join("runner.rs"), &generated.runner);
    write_if_changed(&out.join("metadata.rs"), &generated.metadata);
    if profile {
        println!(
            "cargo:warning={package}: build-profile total_ms={} reuse_2k_total_slots={} reuse_2k_unique_slots={} reuse_64k_total_slots={} reuse_64k_unique_slots={} static_micro_op_bytes={} static_micro_op_instructions={}",
            started.elapsed().as_millis(),
            generated.reuse_2k.total_semantic_word_slots,
            generated.reuse_2k.unique_semantic_word_slots,
            generated.reuse_64k.total_semantic_word_slots,
            generated.reuse_64k.unique_semantic_word_slots,
            generated.static_micro_op_bytes,
            generated.static_micro_op_instructions,
        );
    }
}
