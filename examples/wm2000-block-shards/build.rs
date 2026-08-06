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

const ROM_START: usize = 0x1000;
const BOOT_BYTES: usize = 0x10_0000;
const VA_START: u32 = 0x8000_0400;
const SHARD_BYTES: usize = 64 * 1024;
// The artifact boundary remains 64 KiB, while static subrunners keep rustc
// below the measured memory ceiling. Transfers leave through BlockProgram.
const RUNNER_BYTES: usize = 2 * 1024;

pub const PACKAGES: [&str; 35] = [
    "wm2000-block-overlay-0-shard-00",
    "wm2000-block-overlay-0-shard-01",
    "wm2000-block-overlay-0-shard-02",
    "wm2000-block-overlay-1-shard-00",
    "wm2000-block-overlay-2-shard-00",
    "wm2000-block-overlay-2-shard-01",
    "wm2000-block-overlay-2-shard-02",
    "wm2000-block-overlay-2-shard-03",
    "wm2000-block-overlay-2-shard-04",
    "wm2000-block-overlay-2-shard-05",
    "wm2000-block-overlay-3-shard-00",
    "wm2000-block-overlay-3-shard-01",
    "wm2000-block-overlay-3-shard-02",
    "wm2000-block-overlay-3-shard-03",
    "wm2000-block-overlay-3-shard-04",
    "wm2000-block-overlay-3-shard-05",
    "wm2000-block-overlay-3-shard-06",
    "wm2000-block-overlay-3-shard-07",
    "wm2000-block-resident-tail-shard-00",
    "wm2000-block-resident-tail-shard-01",
    "wm2000-block-shard-00",
    "wm2000-block-shard-01",
    "wm2000-block-shard-02",
    "wm2000-block-shard-03",
    "wm2000-block-shard-04",
    "wm2000-block-shard-05",
    "wm2000-block-shard-06",
    "wm2000-block-shard-07",
    "wm2000-block-shard-08",
    "wm2000-block-shard-09",
    "wm2000-block-shard-10",
    "wm2000-block-shard-11",
    "wm2000-block-shard-12",
    "wm2000-block-shard-13",
    "wm2000-block-shard-14",
];

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

pub struct WmShardGenerator {
    rom: fn64_discover::rom::NormalizedRom,
    overlay_recipes: Option<Vec<OverlayLoadRecipeV1>>,
    host_calls: Vec<u32>,
}

impl WmShardGenerator {
    pub fn from_rom_bytes(source: &[u8]) -> Self {
        let rom = fn64_discover::normalize(source).expect("normalizing shard ROM input");
        let resident_signature = rom.bytes[ROM_START..ROM_START + BOOT_BYTES]
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        let host_bindings = fn64_discover::host_bindings::discover_wm_block_runtime_host_bindings(
            &resident_signature,
            VA_START,
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
                        verify_live_words: true,
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
        let _ = writeln!(
            metadata,
            "pub const SOURCE_SHA256: [u8; 32] = {source_sha256:?};"
        );
        let _ = writeln!(
            metadata,
            "pub const RUNNER_SOURCE_SHA256: [u8; 32] = {runner_source_sha256:?};"
        );
        let _ = write!(metadata, "pub static WORDS: &[u32] = &[");
        for word in words {
            let _ = write!(metadata, "{word:#010X}, ");
        }
        let _ = writeln!(metadata, "];");
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
            PackageTarget::Boot(index @ 0..=13) => (
                Generation {
                    name: "boot".to_string(),
                    source_start: ROM_START,
                    source_end: ROM_START + BOOT_BYTES,
                    affine_source_end: ROM_START + BOOT_BYTES,
                    va_start: VA_START,
                },
                index,
            ),
            PackageTarget::Boot(14) => {
                let first_overlay_start = self.first_overlay_start();
                assert!(
                    VA_START + 14 * (SHARD_BYTES as u32) < first_overlay_start
                        && first_overlay_start <= VA_START + 15 * (SHARD_BYTES as u32),
                    "first overlay invalidation no longer belongs to static-prefix shard 14"
                );
                (
                    Generation {
                        name: "boot".to_string(),
                        source_start: ROM_START,
                        source_end: ROM_START
                            + usize::try_from(first_overlay_start - VA_START)
                                .expect("static-prefix length fits usize"),
                        affine_source_end: ROM_START + BOOT_BYTES,
                        va_start: VA_START,
                    },
                    14,
                )
            }
            PackageTarget::Boot(index) => {
                panic!("static boot shard index {index} is outside the exact prefix")
            }
            PackageTarget::ResidentTail(index) => {
                let first_overlay_start = self.first_overlay_start();
                let source_start = ROM_START
                    + usize::try_from(first_overlay_start - VA_START)
                        .expect("resident-tail source offset fits usize");
                assert_eq!(
                    (ROM_START + BOOT_BYTES - source_start).div_ceil(SHARD_BYTES),
                    2,
                    "resident-tail package topology must cover exactly two shards"
                );
                (
                    Generation {
                        name: "resident_tail".to_string(),
                        source_start,
                        source_end: ROM_START + BOOT_BYTES,
                        affine_source_end: ROM_START + BOOT_BYTES,
                        va_start: first_overlay_start,
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
                // Rounded up to whole shards for the SHARD list only; the
                // generation itself may now end mid-shard, so `image_end`
                // stays at text_end and the trailing shard overhangs.
                const SHARD_BYTES: u32 = 64 * 1024;
                let text_span = (recipe.text_end - recipe.load_start)
                    .div_ceil(SHARD_BYTES)
                    * SHARD_BYTES;
                let source_end = (recipe.rom_start + text_span).min(recipe.rom_end) as usize;
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
