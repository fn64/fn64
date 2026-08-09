//! Build script: runs fn64-discover's real pipeline on the user's own WM2000
//! (NWXE) ROM (env var `ROM`, out-of-tree, never vendored), materializes the
//! admitted Block Pack, and writes two generated files into `OUT_DIR`:
//!
//! - `runner.rs` -- bounded runners for captured CPU-written exception images.
//! - `pack.rs` -- the dense resident/overlay artifact inventory and boot-bank
//!   ROM copy window, as plain consts the runtime harness installs without
//!   re-running discovery.
//! - `FN64_EXECUTABLE_IMAGE_GROUPS` -- comma-separated environment-variable
//!   names, each containing at least three ROM-bound captures of one
//!   CPU-written exception image. Ordinary resident and overlay code comes
//!   only from mechanical ROM discovery; trace-derived generations are
//!   admitted only at the modeled exception-vector entries.
//!
//! Everything here derives from the user's ROM at build time and lands only
//! under `target/` -- no game bytes are committed, matching
//! `../wm2000-boot`'s posture.

use std::env;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const BOOT_SHARD_BYTES: usize = 64 * 1024;
/// Same shard granularity as the `u32` the recipe extents use.
const SHARD_BYTES_U32: u32 = 64 * 1024;
/// The one shard inventory, shared verbatim with the shard generator, the
/// prepared materializer and the verifier. See
/// `../wm2000-block-shards/shard_inventory.in`.
const SHARD_INVENTORY: &[(&str, &str)] =
    &include!("../nomercy-block-shards/shard_inventory.in");
const SHARD_COUNT: usize = SHARD_INVENTORY.len();
const PREPARED_PACKAGES: [&str; SHARD_COUNT] = {
    let mut packages = [""; SHARD_COUNT];
    let mut index = 0;
    while index < SHARD_COUNT {
        packages[index] = SHARD_INVENTORY[index].0;
        index += 1;
    }
    packages
};

struct PreparedCandidateReceipts {
    source_mode: String,
    normalized_rom_sha256: [u8; 32],
    manifest_sha256: [u8; 32],
    tree_sha256: [u8; 32],
    generator_source_sha256: [u8; 32],
    discovery_source_sha256: [u8; 32],
    emitter_source_sha256: [u8; 32],
    runtime_source_sha256: [u8; 32],
    materializer_source_sha256: [u8; 32],
    producer_manifest_sha256: [u8; 32],
    producer_lock_sha256: [u8; 32],
    producer_cargo_graph_sha256: [u8; 32],
    producer_cargo_source_sha256: [u8; 32],
    producer_binary_sha256: [u8; 32],
}

fn decode_digest(value: &str, label: &str) -> [u8; 32] {
    assert!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            && value != "0".repeat(64),
        "{label} must be canonical nonzero lowercase SHA-256"
    );
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap();
    }
    output
}

fn manifest_digest(line: &str, field: &str) -> [u8; 32] {
    decode_digest(
        line.strip_prefix(field)
            .and_then(|rest| rest.strip_prefix(' '))
            .unwrap_or_else(|| panic!("prepared manifest lacks canonical {field}")),
        field,
    )
}

fn required_env_digest(name: &str) -> [u8; 32] {
    println!("cargo:rerun-if-env-changed={name}");
    decode_digest(
        &env::var(name).unwrap_or_else(|_| panic!("missing {name}")),
        name,
    )
}

fn push_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn prepared_candidate_receipts() -> PreparedCandidateReceipts {
    println!("cargo:rerun-if-env-changed=FN64_WM_PREPARED_SHARD_ROOT");
    println!("cargo:rerun-if-env-changed=FN64_WM_PREPARED_SOURCE_MODE");
    let Some(root) = env::var_os("FN64_WM_PREPARED_SHARD_ROOT").map(PathBuf::from) else {
        return PreparedCandidateReceipts {
            source_mode: "legacy_without_prepared_candidate".to_owned(),
            normalized_rom_sha256: [0; 32],
            manifest_sha256: [0; 32],
            tree_sha256: [0; 32],
            generator_source_sha256: [0; 32],
            discovery_source_sha256: [0; 32],
            emitter_source_sha256: [0; 32],
            runtime_source_sha256: [0; 32],
            materializer_source_sha256: [0; 32],
            producer_manifest_sha256: [0; 32],
            producer_lock_sha256: [0; 32],
            producer_cargo_graph_sha256: [0; 32],
            producer_cargo_source_sha256: [0; 32],
            producer_binary_sha256: [0; 32],
        };
    };
    assert!(root.is_absolute(), "prepared root must be absolute");
    let source_mode = env::var("FN64_WM_PREPARED_SOURCE_MODE")
        .expect("verifier-prepared build requires an exact source mode");
    assert!(matches!(
        source_mode.as_str(),
        "legacy_with_prepared_candidate" | "prepared_consumed"
    ));
    let expected_root = std::iter::once("manifest.v2".to_owned())
        .chain(
            PREPARED_PACKAGES
                .iter()
                .map(|package| (*package).to_owned()),
        )
        .collect::<std::collections::BTreeSet<_>>();
    let observed_root = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        observed_root, expected_root,
        "prepared root topology differs"
    );
    let manifest_path = root.join("manifest.v2");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = std::fs::read(&manifest_path).expect("reading prepared manifest");
    let manifest_text = std::str::from_utf8(&manifest).expect("prepared manifest UTF-8");
    assert!(manifest_text.ends_with('\n') && !manifest_text.contains('\r'));
    let lines = manifest_text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 7 + PREPARED_PACKAGES.len());
    assert_eq!(lines[0], "schema fn64.wm-prepared-shard-tree.v2");
    assert_eq!(lines[6], format!("artifact_count {}", PREPARED_PACKAGES.len()));
    let mut tree = Sha256::new();
    tree.update(b"fn64.wm-prepared-shard-complete-tree.v1\0");
    push_bytes(&mut tree, b"manifest.v2");
    tree.update((manifest.len() as u64).to_be_bytes());
    tree.update(Sha256::digest(&manifest));
    for (index, package) in PREPARED_PACKAGES.iter().enumerate() {
        let package_root = root.join(package);
        let observed = std::fs::read_dir(&package_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            observed,
            std::collections::BTreeSet::from([
                "identity.v1".to_owned(),
                "metadata.rs".to_owned(),
                "runner.rs".to_owned()
            ])
        );
        let mut files = Vec::new();
        for name in ["identity.v1", "runner.rs", "metadata.rs"] {
            let path = package_root.join(name);
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|error| panic!("reading prepared artifact: {error}"));
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            let label = format!("{package}/{name}");
            push_bytes(&mut tree, label.as_bytes());
            tree.update((bytes.len() as u64).to_be_bytes());
            tree.update(digest);
            files.push((bytes, digest));
        }
        let expected_sidecar = format!(
            "schema fn64.wm-prepared-shard-artifact.v1\npackage {package}\nrunner_sha256 {}\nmetadata_sha256 {}\n",
            hex(files[1].1), hex(files[2].1)
        );
        assert_eq!(files[0].0, expected_sidecar.as_bytes());
        assert_eq!(
            lines[7 + index],
            format!(
                "artifact {package} {} {} {}",
                hex(files[0].1),
                hex(files[1].1),
                hex(files[2].1)
            )
        );
    }
    PreparedCandidateReceipts {
        source_mode,
        normalized_rom_sha256: manifest_digest(lines[1], "normalized_rom_sha256"),
        manifest_sha256: Sha256::digest(&manifest).into(),
        tree_sha256: tree.finalize().into(),
        generator_source_sha256: manifest_digest(lines[2], "generator_source_sha256"),
        discovery_source_sha256: manifest_digest(lines[3], "discovery_source_sha256"),
        emitter_source_sha256: manifest_digest(lines[4], "emitter_source_sha256"),
        runtime_source_sha256: manifest_digest(lines[5], "runtime_source_sha256"),
        materializer_source_sha256: required_env_digest(
            "FN64_WM_PREPARED_MATERIALIZER_SOURCE_SHA256",
        ),
        producer_manifest_sha256: required_env_digest("FN64_WM_PREPARED_PRODUCER_MANIFEST_SHA256"),
        producer_lock_sha256: required_env_digest("FN64_WM_PREPARED_PRODUCER_LOCK_SHA256"),
        producer_cargo_graph_sha256: required_env_digest(
            "FN64_WM_PREPARED_PRODUCER_CARGO_GRAPH_SHA256",
        ),
        producer_cargo_source_sha256: required_env_digest(
            "FN64_WM_PREPARED_PRODUCER_CARGO_SOURCE_SHA256",
        ),
        producer_binary_sha256: required_env_digest("FN64_WM_PREPARED_PRODUCER_BINARY_SHA256"),
    }
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_if_changed(path: &Path, contents: &str) {
    if std::fs::read(path).is_ok_and(|existing| existing == contents.as_bytes()) {
        return;
    }
    std::fs::write(path, contents).unwrap();
}

fn source_tree_sha256(domain: &[u8], files: &[PathBuf]) -> [u8; 32] {
    let mut files = files.to_vec();
    files.sort();
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let label = path.to_string_lossy();
        let source = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("reading Cargo source {}: {error}", path.display()));
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label.as_bytes());
        hasher.update((source.len() as u64).to_be_bytes());
        hasher.update(source);
    }
    hasher.finalize().into()
}

fn cargo_source_receipts() -> ([u8; 32], [u8; 32], [u8; 32], [u8; 32]) {
    let root_adapter_source_sha256 = source_tree_sha256(
        b"fn64:wm2000-root-adapter-source:v1:",
        &[
            PathBuf::from("Cargo.toml"),
            PathBuf::from("Cargo.lock"),
            PathBuf::from("build.rs"),
            PathBuf::from("src/main.rs"),
        ],
    );
    let shard_root = PathBuf::from("../nomercy-block-shards");
    let mut shard_sources = vec![shard_root.join("lib.rs")];
    match env::var("FN64_WM_PREPARED_SOURCE_MODE").as_deref() {
        Ok("prepared_consumed") => {
            shard_sources.push(shard_root.join("prepared_build.rs"));
            shard_sources.push(shard_root.join("materializer.rs"));
        }
        Ok("legacy_with_prepared_candidate") | Err(_) => {
            shard_sources.push(shard_root.join("build.rs"));
        }
        Ok(mode) => panic!("unsupported WM prepared source mode {mode}"),
    }
    let mut manifests = std::fs::read_dir(&shard_root)
        .expect("reading generated shard source root")
        .map(|entry| {
            entry
                .expect("reading generated shard source entry")
                .path()
                .join("Cargo.toml")
        })
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    manifests.sort();
    assert_eq!(
        manifests.len(),
        SHARD_COUNT,
        "trusted WM generated source graph must contain exactly {SHARD_COUNT} shard manifests"
    );
    shard_sources.extend(manifests);
    let shard_cargo_source_tree_sha256 =
        source_tree_sha256(b"fn64:wm2000-shard-cargo-source-tree:v1:", &shard_sources);
    let emitter_source_sha256 =
        fn64_recomp_rs_codegen::generated_runner_emitter_source_receipt_v2().source_sha256();
    let runtime_source_sha256 =
        fn64_recomp_rs::generated_runner_runtime_source_receipt_v1().source_sha256();
    (
        root_adapter_source_sha256,
        shard_cargo_source_tree_sha256,
        emitter_source_sha256,
        runtime_source_sha256,
    )
}

fn reproducible_executable_image_from_env(
    normalized_rom_sha256: &str,
    env_name: &str,
) -> fn64_discover::trace::ExecutableImageCapture {
    println!("cargo:rerun-if-env-changed={env_name}");
    let paths = env::var_os(env_name)
        .map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        paths.len() >= 3,
        "wm2000-block-boot build.rs: {env_name} must contain at least three paths, separated with the platform path separator"
    );
    let expected =
        fn64_discover::trace::NormalizedRomDigest::try_from(normalized_rom_sha256.to_string())
            .expect("discovery produced a canonical normalized-ROM digest");
    let documents = paths
        .iter()
        .map(|path| {
            println!("cargo:rerun-if-changed={}", path.display());
            std::fs::read(path).unwrap_or_else(|error| {
                panic!(
                    "wm2000-block-boot build.rs: reading executable image {}: {error}",
                    path.display()
                )
            })
        })
        .collect::<Vec<_>>();
    fn64_discover::trace::parse_reproducible_executable_image_group(&documents, &expected, 3)
        .unwrap_or_else(|error| {
            panic!(
                "wm2000-block-boot build.rs: validating reproducible executable-image group {env_name}: {error}"
            )
        })
}

fn reproducible_executable_images_from_env(
    normalized_rom_sha256: &str,
) -> Vec<fn64_discover::trace::ExecutableImageCapture> {
    println!("cargo:rerun-if-env-changed=FN64_EXECUTABLE_IMAGE_GROUPS");
    let group_names = env::var("FN64_EXECUTABLE_IMAGE_GROUPS")
        .unwrap_or_else(|_| "FN64_EXECUTABLE_IMAGES".to_string())
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(
        !group_names.is_empty(),
        "wm2000-block-boot build.rs: FN64_EXECUTABLE_IMAGE_GROUPS names no capture groups"
    );
    let captures = group_names
        .iter()
        .map(|name| reproducible_executable_image_from_env(normalized_rom_sha256, name))
        .collect::<Vec<_>>();
    for (index, capture) in captures.iter().enumerate() {
        assert!(
            !captures[..index].iter().any(|known| {
                known.image_id == capture.image_id && known.generation == capture.generation
            }),
            "wm2000-block-boot build.rs: duplicate executable-image identity {} generation {}",
            capture.image_id,
            capture.generation
        );
    }
    captures
}

fn main() {
    println!("cargo:rerun-if-changed=src/main.rs");
    println!("cargo:rerun-if-env-changed=ROM");
    let manifest_sha256: [u8; 32] = Sha256::digest(
        std::fs::read("Cargo.toml").expect("reading WM generated-runner Cargo manifest"),
    )
    .into();
    let lock_sha256: [u8; 32] = Sha256::digest(
        std::fs::read("Cargo.lock").expect("reading WM generated-runner Cargo lockfile"),
    )
    .into();
    let (
        root_adapter_source_sha256,
        shard_cargo_source_tree_sha256,
        emitter_source_sha256,
        runtime_source_sha256,
    ) = cargo_source_receipts();
    let rom_path = env::var("ROM").unwrap_or_else(|_| {
        panic!(
            "wm2000-block-boot build.rs: required environment variable ROM is not set.\n\
             Point it at your own legally-obtained WM2000 (NWXE) ROM file. This crate \
             contains zero game content; the discovered pack is derived at build time \
             and never leaves target/."
        )
    });
    println!("cargo:rerun-if-changed={rom_path}");
    let rom_bytes = std::fs::read(&rom_path)
        .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: reading ROM {rom_path}: {e}"));

    let (rom, db) = fn64_discover::run_discovery(&rom_bytes, None)
        .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: ROM rejected: {e:?}"));
    let overlay_search = fn64_discover::overlay_regions::SearchConfig::aki_family();
    let overlay_recovery = fn64_discover::overlay_regions::recover_overlay_regions(
        &rom.bytes,
        &overlay_search,
        &fn64_discover::delta_vote::DeltaVoteConfig::default(),
        overlay_search.min_records,
    );
    let overlay_recipes = fn64_discover::overlay_recipe::admitted_overlay_load_recipes_v1(
        &rom.bytes,
        &overlay_recovery,
    )
    .expect("wm2000-block-boot build.rs: recovering one complete overlay recipe table");
    // Overlay COUNT is a property of the ROM, not of this lane: discovery
    // recovers 4 for WM2000, 5 for No Mercy, 2 for Revenge and World Tour, and
    // 4 for VPW2. Pinning it at 4 made the lane WM2000-only for no reason --
    // every geometry below is already derived from `overlay_recipes` itself.
    // What the lane genuinely requires is at least one recovered overlay.
    assert!(
        !overlay_recipes.is_empty(),
        "wm2000-block-boot build.rs: closure requires at least one recovered overlay generation"
    );
    let overlay_names = (0..overlay_recipes.len())
        .map(|index| format!("recovered_overlay_{index}"))
        .collect::<Vec<_>>();
    let overlay_dense_inputs = overlay_names
        .iter()
        .zip(&overlay_recipes)
        .map(|(name, recipe)| {
            fn64_discover::dense_aot_pack::DenseAotGenerationInput::from((name.as_str(), recipe))
        })
        .collect::<Vec<_>>();
    let overlay_dense_pack =
        fn64_discover::dense_aot_pack::build_dense_aot_pack_v1(&rom, &overlay_dense_inputs)
            .expect("wm2000-block-boot build.rs: building overlay dense-AOT manifest");
    let boot_mapping = db
        .proven_rom_mappings()
        .into_iter()
        .find_map(|fact| match fact {
            fn64_discover::Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if *bank == fn64_discover::banks::BOOT_BANK => {
                Some((*rom_start, *rom_end, *va_start))
            }
            _ => None,
        })
        .expect("wm2000-block-boot build.rs: boot bank not proven by discovery");
    let (rom_start, rom_end, va_start) = boot_mapping;
    let bank_bytes = &rom.bytes[rom_start as usize..rom_end as usize];
    let entrypoint = rom.header.entry_point;

    let executable_images = reproducible_executable_images_from_env(&rom.sha256);

    let input = fn64_discover::snapshot::MaterializedBankInput {
        bank: fn64_discover::banks::BOOT_BANK,
        va_start,
        bytes: bank_bytes,
        seed_roots: std::slice::from_ref(&entrypoint),
    };
    let snapshot =
        fn64_discover::snapshot::compose_materialized_bank_validated_v2(&rom, &db, input)
            .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: composing snapshot: {e}"));
    let block_pack = fn64_discover::block_pack::emit_validated_block_pack_v2(&snapshot, 0, &rom)
        .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: emitting Block Pack: {e}"));
    let mut materialized = fn64_discover::block_pack::materialize_block_pack(&block_pack, &rom)
        .unwrap_or_else(|e| panic!("wm2000-block-boot build.rs: materializing Block Pack: {e}"));
    materialized[0].blocks = vec![fn64_discover::block_pack::MaterializedPackedBlock {
        start_va: va_start,
        words: bank_bytes
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
            .collect(),
    }];
    let dense_bank = &materialized[0];
    let dense_words = &dense_bank.blocks[0].words;
    let resident_image_start = overlay_recipes
        .iter()
        .map(|recipe| recipe.load_start)
        .min()
        .expect("recovered overlay catalog is nonempty");
    let resident_image_end = va_start
        .checked_add(u32::try_from(bank_bytes.len()).expect("boot bank length fits u32"))
        .expect("boot bank virtual range does not overflow");
    assert!(
        va_start < resident_image_start && resident_image_start < resident_image_end,
        "wm2000-block-boot build.rs: first overlay invalidation must split the resident boot bank"
    );
    let invalidation_start = resident_image_start;
    let invalidation_end = overlay_recipes
        .iter()
        .map(|recipe| recipe.bss_end)
        .max()
        .expect("recovered overlay catalog is nonempty");
    // THE ASSERTION THAT USED TO BE HERE WAS STALE, AND IT REJECTED THIS ROM.
    //
    // It required `invalidation_end >= resident_image_end`. `resident_image_end`
    // is not a discovered code extent -- it is `va_start + BOOT_COPY_SIZE`, the
    // fixed 1 MiB IPL3 boot DMA, identical for every ROM this path admits.
    // Nothing makes a game's overlays reach that hardware constant. WM2000's do
    // (union 0x80171a60 past a resident end of 0x80100400), which is the only
    // reason the requirement went unnoticed; Revenge's stop at 0x800fafa0,
    // 21,600 bytes short, and the build died here before compiling a line.
    //
    // `fn64-discover` had ALREADY FIXED THIS. `generation_topology/mod.rs:448-477`
    // names WCW/nWo Revenge and WCW vs nWo World Tour explicitly, records the
    // same two numbers, and clamps `tail_image_end = load_end.min(union_end)` --
    // calling the unclamped fold "the actual error". So the library accepts this
    // geometry and this build script rejected it first: a stale precondition in
    // front of a corrected implementation.
    //
    // Measured on the uncovered span, to check the clamp is not discarding code
    // (ROM d8c097f8880032fc..., rom [0xfbba0,0x101000), va [0x800fafa0,0x80100400)):
    //
    //   region                       common-MIPS opcodes   undefined opcodes
    //   Revenge resident boot text          78.5%                 0.6%
    //   Revenge overlay-1 text              74.8%                 0.3%
    //   THE UNCOVERED TAIL                  20.1%                46.0%
    //   WM2000 at the same rom offset       71.8%                 0.8%
    //
    // The span is DATA, not code -- and WM2000's being code at the same offset
    // is precisely why the two titles land on opposite sides of the old rule.
    //
    // What replaces the assertion is the library's own clamp, mirrored here so
    // the `resident_tail_generation_id` this file computes is derived from the
    // same extent `build_generation_topology_v1` will use. Leaving the digest
    // over the unclamped bank tail while the library clamps would produce two
    // different generation ids for one generation -- a silent identity split,
    // which is worse than the loud assertion this replaces. Clamping only ever
    // SHRINKS the image, so it is fail-closed; an empty tail is still rejected,
    // by `EmptyResidentTail` in the library and by the assertion below here.
    let resident_tail_image_end = resident_image_end.min(invalidation_end);
    assert!(
        resident_image_start < resident_tail_image_end,
        "wm2000-block-boot build.rs: overlays and the resident bank contend for no byte: \
         split {resident_image_start:#010x} is at or past the invalidation union end \
         {invalidation_end:#010x}"
    );
    let resident_byte_offset =
        usize::try_from(resident_image_start - va_start).expect("resident-tail offset fits usize");
    assert_eq!(
        resident_byte_offset % 4,
        0,
        "wm2000-block-boot build.rs: resident-tail boundary is instruction-aligned"
    );
    let resident_tail_byte_end = usize::try_from(resident_tail_image_end - va_start)
        .expect("resident-tail end offset fits usize");
    let resident_tail_bytes = &bank_bytes[resident_byte_offset..resident_tail_byte_end];
    // How many 64 KiB shards the resident-tail generation has. Derived the way
    // `runtime_generation_catalog.rs:123-126` derives it -- `div_ceil` over the
    // CLAMPED image -- so the pack and the library agree by construction rather
    // than by coincidence. The shard generator applies the same clamp, so this
    // is also the number of compiled resident-tail packages.
    let resident_tail_generation_shard_count = usize::try_from(
        (resident_tail_image_end - resident_image_start).div_ceil(SHARD_BYTES_U32),
    )
    .expect("resident-tail generation shard count fits usize");
    let resident_tail_sha256: [u8; 32] = Sha256::digest(resident_tail_bytes).into();
    // Argument order and values mirror the library's own call at
    // `generation_topology/mod.rs:514-522` -- `(split, tail_image_end, split,
    // invalidation_end, tail_digest)`. The second argument is the CLAMPED end,
    // not `resident_image_end`: on WM2000 the two are equal because its
    // invalidation union runs past the boot bank, so the distinction never
    // showed; on Revenge they differ by 21,600 bytes and passing the unclamped
    // value would derive a different generation id from the one the library
    // derives for the same generation.
    let resident_tail_generation_id =
        fn64_discover::generation_topology::resident_tail_generation_id_v1(
            b"fn64:wm2000-resident-tail-generation:v1:",
            &rom.sha256,
            resident_image_start,
            resident_tail_image_end,
            invalidation_start,
            invalidation_end,
            resident_tail_sha256,
        );
    let mut combined_dense_inputs = vec![fn64_discover::dense_aot_pack::DenseAotGenerationInput {
        name: fn64_discover::banks::BOOT_BANK,
        source_rom_start: rom_start,
        source_rom_end: rom_end,
        load_start: va_start,
        text_start: va_start,
        text_end: resident_image_end,
        data_start: resident_image_end,
        data_end: resident_image_end,
        bss_start: resident_image_end,
        bss_end: resident_image_end,
    }];
    combined_dense_inputs.extend(overlay_names.iter().zip(&overlay_recipes).map(
        |(name, recipe)| {
            fn64_discover::dense_aot_pack::DenseAotGenerationInput::from((name.as_str(), recipe))
        },
    ));
    let combined_dense_pack =
        fn64_discover::dense_aot_pack::build_dense_aot_pack_v1(&rom, &combined_dense_inputs)
            .expect("wm2000-block-boot build.rs: building combined dense generation pack");
    let generation_topology = fn64_discover::generation_topology::build_generation_topology_v1(
        &rom,
        &combined_dense_pack,
        fn64_discover::banks::BOOT_BANK,
        b"fn64:wm2000-resident-tail-generation:v1:",
        &overlay_recipes,
    )
    .expect("wm2000-block-boot build.rs: building dense generation topology");
    let dense_generation_catalog =
        fn64_discover::runtime_generation_catalog::build_backed_dense_generation_catalog_v1(
            &rom,
            &combined_dense_pack,
            &generation_topology,
        )
        .expect("wm2000-block-boot build.rs: building canonical dense generation catalog");
    let dense_generation_catalog_definition_sha256 =
        dense_generation_catalog.canonical_definition_sha256();
    let resident_host_bindings =
        fn64_discover::host_bindings::discover_wm_block_runtime_host_bindings(
            dense_words,
            va_start,
        )
        .expect("wm2000-block-boot build.rs: discovering exact runtime host catalog");
    let guest_thread_globals =
        fn64_discover::host_bindings::discover_guest_thread_globals(dense_words, va_start)
            .expect("wm2000-block-boot build.rs: discovering guest thread globals");
    // Optional: a cartridge-only title has no 64DD drive-init routine at all,
    // so absence is normal and only an ambiguous shape is an error.
    let drive_rom_init =
        fn64_discover::host_bindings::discover_drive_rom_init_host_binding(dense_words, va_start)
            .expect("wm2000-block-boot build.rs: discovering 64DD drive init");
    let binding_address = |symbol| {
        resident_host_bindings
            .iter()
            .find(|binding| binding.symbol == symbol)
            .expect("required resident host binding is present")
            .vram
    };
    let os_si_device_busy =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsSiDeviceBusy);
    let os_create_mesg_queue =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsCreateMesgQueue);
    let os_create_thread =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsCreateThread);
    let os_epi_start_dma =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsEPiStartDma);
    let os_get_thread_pri =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsGetThreadPri);
    let os_recv_mesg = binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsRecvMesg);
    let os_send_mesg = binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsSendMesg);
    let os_set_event_mesg =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsSetEventMesg);
    let os_set_thread_pri =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsSetThreadPri);
    let os_set_timer = binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsSetTimer);
    let os_sp_task_load =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsSpTaskLoad);
    let os_sp_task_start_go =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsSpTaskStartGo);
    let os_sp_task_yield =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsSpTaskYield);
    let os_sp_task_yielded =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsSpTaskYielded);
    let os_start_thread =
        binding_address(fn64_discover::host_bindings::HostBindingSymbol::OsStartThread);
    let mut runner = String::new();
    let boot_shards = dense_words[..resident_byte_offset / 4]
        .chunks(BOOT_SHARD_BYTES / 4)
        .enumerate()
        .map(|(index, words)| {
            let start_va = va_start
                + u32::try_from(index * BOOT_SHARD_BYTES).expect("boot shard VA offset fits u32");
            fn64_discover::block_pack::MaterializedPackedBank {
                bank: format!("{}:shard:{index:02}", fn64_discover::banks::BOOT_BANK),
                bank_id: fn64_discover::dense_aot_pack::dense_aot_artifact_bank_id(
                    &rom.sha256,
                    "boot",
                    start_va,
                    words,
                ),
                blocks: vec![fn64_discover::block_pack::MaterializedPackedBlock {
                    start_va,
                    words: words.to_vec(),
                }],
            }
        })
        .collect::<Vec<_>>();
    // THE COUNT COMES FROM THE CLAMPED IMAGE; THE SHARDS THEMSELVES ARE WHOLE
    // 64 KiB BLOCKS AND THE LAST ONE MAY OVERHANG. These are two different
    // things and conflating them is its own digest bug, distinct from the
    // clamp itself.
    //
    // `runtime_generation_catalog.rs:117-126` spells the rule out: the shard
    // span is `byte_len.div_ceil(SHARD)*SHARD` capped at `source_rom_end`, and
    // its comment warns that chunking to `image_end` instead "would emit a
    // final shard that stops at `image_end` and disagree with the shard
    // geometry the pack emits, which is exactly the catalog-digest mismatch
    // this produced." Truncating here reproduced that mismatch exactly --
    // `block_program.rs:345`, runtime catalog != build-time definition -- with
    // correct geometry on every generation and only the final shard's `end`
    // differing (0x800fafa0 truncated against 0x80100000 whole).
    //
    // `PrecompiledGeneration::new` permits the overhang deliberately
    // (`fn64-recomp-rs/src/generation/mod.rs:109-126`): the generation's digest
    // covers `[image_start, image_end)` only, so bytes past `image_end` inside
    // the final shard are not part of its identity.
    //
    // So: take `generation_tail_shard_count` whole blocks, capped at the boot
    // copy's end. For Revenge that is 7 blocks ending at 0x80100000 -- the
    // 0x400-byte remainder of the bank belongs to no generation, which is the
    // ownership the library assigns it.
    let resident_tail_shard_span_end = (resident_byte_offset
        + resident_tail_generation_shard_count * BOOT_SHARD_BYTES)
        .min(dense_words.len() * 4);
    let resident_tail_shards = dense_words
        [resident_byte_offset / 4..resident_tail_shard_span_end / 4]
        .chunks(BOOT_SHARD_BYTES / 4)
        .enumerate()
        .map(|(index, words)| {
            let start_va = resident_image_start
                + u32::try_from(index * BOOT_SHARD_BYTES)
                    .expect("resident-tail shard VA offset fits u32");
            fn64_discover::block_pack::MaterializedPackedBank {
                bank: format!("resident_tail:shard:{index:02}"),
                bank_id: fn64_discover::dense_aot_pack::dense_aot_artifact_bank_id(
                    &rom.sha256,
                    "resident_tail",
                    start_va,
                    words,
                ),
                blocks: vec![fn64_discover::block_pack::MaterializedPackedBlock {
                    start_va,
                    words: words.to_vec(),
                }],
            }
        })
        .collect::<Vec<_>>();
    // Shard counts follow from the boot bank size and where the FIRST overlay
    // loads, both of which are per-ROM: WM2000 splits 15 + 2, other titles
    // split elsewhere. The invariant the lane actually needs is that the two
    // halves together tile the bank and that the prepared package inventory
    // has a shard for each, which the topology checks below enforce.
    // Derived from this crate's own package name rather than written as a
    // literal. With WM2000's literal left in a Revenge tree this filter
    // matches nothing and `expected_total_shards` is 0 -- an UNDER-match, the
    // dangerous direction: the assert below then reports "17 boot + 10 tail vs
    // 0 prepared" and reads as a topology defect in the ROM rather than as a
    // wrong prefix string. Deriving it makes a prefix mismatch impossible
    // instead of merely detectable.
    let package_prefix = env!("CARGO_PKG_NAME")
        .strip_suffix("-boot")
        .expect("this crate is named <prefix>-boot");
    let boot_shard_prefix = format!("{package_prefix}-shard-");
    let resident_tail_shard_prefix = format!("{package_prefix}-resident-tail-shard-");
    let expected_total_shards = PREPARED_PACKAGES
        .iter()
        .filter(|name| {
            name.starts_with(&boot_shard_prefix)
                || name.starts_with(&resident_tail_shard_prefix)
        })
        .count();
    assert!(
        expected_total_shards > 0,
        "no prepared package carries the resident prefixes {boot_shard_prefix:?} / \
         {resident_tail_shard_prefix:?}; the shard inventory belongs to another title"
    );
    assert_eq!(
        boot_shards.len() + resident_tail_shards.len(),
        expected_total_shards,
        "resident topology must tile the boot bank across the prepared packages: \
         {} boot + {} tail vs {expected_total_shards} prepared",
        boot_shards.len(),
        resident_tail_shards.len(),
    );
    assert!(
        !boot_shards.is_empty() && !resident_tail_shards.is_empty(),
        "resident split must produce both a static prefix and a resident tail"
    );
    assert_eq!(
        boot_shards.last().map(|shard| {
            let block = &shard.blocks[0];
            block.start_va + u32::try_from(block.words.len() * 4).unwrap()
        }),
        Some(resident_image_start),
        "static prefix ends at the first overlay invalidation"
    );
    assert_eq!(
        resident_tail_shards
            .first()
            .map(|shard| shard.blocks[0].start_va),
        Some(resident_image_start),
        "resident-tail shard cover starts at its image boundary"
    );
    assert_eq!(
        resident_tail_shards.last().map(|shard| {
            let block = &shard.blocks[0];
            block.start_va + u32::try_from(block.words.len() * 4).unwrap()
        }),
        Some(
            va_start
                + u32::try_from(resident_tail_shard_span_end)
                    .expect("resident-tail shard span end fits u32")
        ),
        "resident-tail shard cover ends at its whole-block span end, which may \
         OVERHANG the clamped image end (the last shard is a full 64 KiB block)"
    );
    for shards in [&boot_shards, &resident_tail_shards] {
        for pair in shards.windows(2) {
            let first = &pair[0].blocks[0];
            let second = &pair[1].blocks[0];
            assert_eq!(
                first.start_va + u32::try_from(first.words.len() * 4).unwrap(),
                second.start_va,
                "dense resident shard cover is contiguous"
            );
        }
    }

    let mut immutable_ranges = boot_shards
        .iter()
        .chain(&resident_tail_shards)
        .map(|shard| {
            let block = &shard.blocks[0];
            fn64_discover::external_aot::ImmutableAotRange {
                label: shard.bank.clone(),
                bank_id: shard.bank_id,
                va_start: block.start_va,
                va_end: block.start_va + block.words.len() as u32 * 4,
            }
        })
        .collect::<Vec<_>>();
    for (name, recipe) in overlay_names.iter().zip(&overlay_recipes) {
        let source_rom_end =
            recipe.rom_start + fn64_discover::overlay_recipe::generation_source_span(recipe);
        let source = &rom.bytes[recipe.rom_start as usize..source_rom_end as usize];
        for (shard_index, bytes) in source.chunks(BOOT_SHARD_BYTES).enumerate() {
            let words = bytes
                .chunks_exact(4)
                .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                .collect::<Vec<_>>();
            let va_start = recipe.load_start
                + u32::try_from(shard_index * BOOT_SHARD_BYTES)
                    .expect("overlay shard VA offset fits u32");
            immutable_ranges.push(fn64_discover::external_aot::ImmutableAotRange {
                label: format!("{name}:shard:{shard_index:02}"),
                bank_id: fn64_discover::dense_aot_pack::dense_aot_artifact_bank_id(
                    &rom.sha256,
                    name,
                    va_start,
                    &words,
                ),
                va_start,
                va_end: va_start + u32::try_from(bytes.len()).unwrap(),
            });
        }
    }
    let normalized_rom_digest =
        fn64_discover::trace::NormalizedRomDigest::try_from(rom.sha256.clone())
            .expect("normalized ROM SHA-256 is canonical");
    let external_catalog = fn64_discover::external_aot::build_external_aot_catalog(
        &normalized_rom_digest,
        &executable_images,
        &fn64_discover::source_closure::MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1,
        &immutable_ranges,
    )
    .unwrap_or_else(|error| {
        panic!("wm2000-block-boot build.rs: validating external AOT catalog: {error:?}")
    });
    let vector_banks = external_catalog
        .iter()
        .map(|image| fn64_discover::block_pack::MaterializedPackedBank {
            bank: format!(
                "{}:generation:{}",
                image.capture.image_id, image.capture.generation
            ),
            bank_id: image.bank_id,
            blocks: vec![fn64_discover::block_pack::MaterializedPackedBlock {
                start_va: image.capture.va_start,
                words: image.capture.words.clone(),
            }],
        })
        .collect::<Vec<_>>();
    for (index, vector_bank) in vector_banks.iter().enumerate() {
        runner.push('\n');
        runner.push_str(&fn64_discover::block_pack::emit_materialized_bank_runner(
            vector_bank,
            &format!("run_nwxe_exception_image_{index:02}"),
        ));
    }
    runner.push_str(
        "\npub fn run_nwxe_exception_image(entry: ExecutionKey, budget: InstructionBudget, ctx: &mut RecompContext, mem: &mut Rdram<'_>) -> BlockRun {\n    match entry.bank.get() {\n",
    );
    for (index, vector_bank) in vector_banks.iter().enumerate() {
        let _ = writeln!(
            runner,
            "        {:#018X} => run_nwxe_exception_image_{index:02}(entry, budget, ctx, mem),",
            vector_bank.bank_id
        );
    }
    runner.push_str(
        "        bank => panic!(\"no generated external-image runner for bank {bank:#018x}\"),\n    }\n}\n",
    );
    let external_runner_source_sha256: [u8; 32] = Sha256::digest(runner.as_bytes()).into();
    let dispatch_source =
        std::fs::read("src/main.rs").expect("reading production dispatch source for identity");
    let prepared = prepared_candidate_receipts();
    if prepared.source_mode != "legacy_without_prepared_candidate" {
        assert_eq!(hex(prepared.normalized_rom_sha256), rom.sha256);
        assert_eq!(prepared.emitter_source_sha256, emitter_source_sha256);
        assert_eq!(prepared.runtime_source_sha256, runtime_source_sha256);
    }

    let mut pack = String::new();
    let entry_bank_id = boot_shards
        .iter()
        .find(|shard| {
            let block = &shard.blocks[0];
            (block.start_va..block.start_va + block.words.len() as u32 * 4).contains(&entrypoint)
        })
        .expect("entrypoint belongs to one boot shard")
        .bank_id;
    let _ = writeln!(
        pack,
        "pub const ENTRY_BANK_ID: u64 = {entry_bank_id:#018X};"
    );
    let _ = writeln!(pack, "pub const ENTRYPOINT: u32 = {entrypoint:#010X};");
    // The once-only guard the guest's own 64DD init tests. Presetting it makes
    // that routine take its already-initialised path, which returns the same
    // static OSPiHandle* without probing a drive this cartridge has no device
    // for. `None` for a title with no such routine.
    let _ = match drive_rom_init {
        Some(found) => writeln!(
            pack,
            "pub const DRIVE_ROM_INIT_GUARD: Option<u32> = Some({:#010X});",
            found.guard_vram
        ),
        None => writeln!(pack, "pub const DRIVE_ROM_INIT_GUARD: Option<u32> = None;"),
    };
    let _ = writeln!(
        pack,
        "pub const OS_SI_DEVICE_BUSY: u32 = {os_si_device_busy:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const OS_CREATE_MESG_QUEUE: u32 = {os_create_mesg_queue:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const OS_EPI_START_DMA: u32 = {os_epi_start_dma:#010X};"
    );
    let _ = writeln!(pack, "pub const OS_RECV_MESG: u32 = {os_recv_mesg:#010X};");
    let _ = writeln!(pack, "pub const OS_SEND_MESG: u32 = {os_send_mesg:#010X};");
    let _ = writeln!(
        pack,
        "pub const OS_CREATE_THREAD: u32 = {os_create_thread:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const OS_SET_EVENT_MESG: u32 = {os_set_event_mesg:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const OS_START_THREAD: u32 = {os_start_thread:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const OS_GET_THREAD_PRI: u32 = {os_get_thread_pri:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const OS_RUNNING_THREAD: u32 = {:#010X};",
        guest_thread_globals.running_thread_vram
    );
    let _ = writeln!(
        pack,
        "pub const OS_SET_THREAD_PRI: u32 = {os_set_thread_pri:#010X};"
    );
    let _ = writeln!(pack, "pub const OS_SET_TIMER: u32 = {os_set_timer:#010X};");
    let _ = writeln!(
        pack,
        "pub const OS_SP_TASK_LOAD: u32 = {os_sp_task_load:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const OS_SP_TASK_START_GO: u32 = {os_sp_task_start_go:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const OS_SP_TASK_YIELD: u32 = {os_sp_task_yield:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const OS_SP_TASK_YIELDED: u32 = {os_sp_task_yielded:#010X};"
    );
    let _ = writeln!(
        pack,
        "pub const ROM_COPY: (usize, usize, u32) = ({rom_start:#X}, {rom_end:#X}, {va_start:#010X});"
    );
    let _ = writeln!(
        pack,
        "pub const DENSE_GENERATION_CATALOG_DEFINITION_SHA256: [u8; 32] = {dense_generation_catalog_definition_sha256:?};"
    );
    let _ = writeln!(
        pack,
        "pub struct DenseShard {{ pub bank_id: u64, pub va_start: u32, pub byte_len: u32, pub source_sha256: [u8; 32], pub code_sha256: [u8; 32] }}"
    );
    let _ = writeln!(
        pack,
        "pub const EXTERNAL_RUNNER_SOURCE_SHA256: [u8; 32] = {external_runner_source_sha256:?};"
    );
    let _ = writeln!(
        pack,
        "pub const ROOT_ADAPTER_SOURCE_SHA256: [u8; 32] = {root_adapter_source_sha256:?};"
    );
    let _ = writeln!(
        pack,
        "pub const SHARD_CARGO_SOURCE_TREE_SHA256: [u8; 32] = {shard_cargo_source_tree_sha256:?};"
    );
    let _ = writeln!(
        pack,
        "pub const EMITTER_SOURCE_SHA256: [u8; 32] = {emitter_source_sha256:?};"
    );
    let _ = writeln!(
        pack,
        "pub const RUNTIME_SOURCE_SHA256: [u8; 32] = {runtime_source_sha256:?};"
    );
    let _ = writeln!(
        pack,
        "pub const MANIFEST_SHA256: [u8; 32] = {manifest_sha256:?};"
    );
    let _ = writeln!(pack, "pub const LOCK_SHA256: [u8; 32] = {lock_sha256:?};");
    let _ = writeln!(
        pack,
        "pub const PREPARED_SOURCE_MODE: &str = {:?};",
        prepared.source_mode
    );
    // The normalized-ROM digest is emitted UNCONDITIONALLY, not only in
    // prepared mode. It is the identity a release build checks the user's ROM
    // against at startup, so a build that left it all-zero would silently
    // accept the wrong ROM -- worse than baking the words in. `rom.sha256` is
    // the digest of the normalized big-endian image, which is also the form
    // the shard geometry offsets index.
    let normalized_rom_sha256 = {
        let hex_digits = rom.sha256.as_bytes();
        assert_eq!(
            hex_digits.len(),
            64,
            "normalized ROM digest is a canonical SHA-256"
        );
        let mut digest = [0u8; 32];
        for (index, pair) in hex_digits.chunks_exact(2).enumerate() {
            let nibble = |c: u8| match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => panic!("normalized ROM digest is not hexadecimal"),
            };
            digest[index] = (nibble(pair[0]) << 4) | nibble(pair[1]);
        }
        digest
    };
    if prepared.source_mode != "legacy_without_prepared_candidate" {
        assert_eq!(
            prepared.normalized_rom_sha256, normalized_rom_sha256,
            "prepared receipts and the build ROM must name one image"
        );
    }
    for (name, digest) in [
        ("NORMALIZED_ROM_SHA256", normalized_rom_sha256),
        ("PREPARED_MANIFEST_SHA256", prepared.manifest_sha256),
        ("PREPARED_TREE_SHA256", prepared.tree_sha256),
        (
            "PREPARED_GENERATOR_SOURCE_SHA256",
            prepared.generator_source_sha256,
        ),
        (
            "PREPARED_DISCOVERY_SOURCE_SHA256",
            prepared.discovery_source_sha256,
        ),
        (
            "PREPARED_EMITTER_SOURCE_SHA256",
            prepared.emitter_source_sha256,
        ),
        (
            "PREPARED_RUNTIME_SOURCE_SHA256",
            prepared.runtime_source_sha256,
        ),
        (
            "PREPARED_MATERIALIZER_SOURCE_SHA256",
            prepared.materializer_source_sha256,
        ),
        (
            "PREPARED_PRODUCER_MANIFEST_SHA256",
            prepared.producer_manifest_sha256,
        ),
        (
            "PREPARED_PRODUCER_LOCK_SHA256",
            prepared.producer_lock_sha256,
        ),
        (
            "PREPARED_PRODUCER_CARGO_GRAPH_SHA256",
            prepared.producer_cargo_graph_sha256,
        ),
        (
            "PREPARED_PRODUCER_CARGO_SOURCE_SHA256",
            prepared.producer_cargo_source_sha256,
        ),
        (
            "PREPARED_PRODUCER_BINARY_SHA256",
            prepared.producer_binary_sha256,
        ),
    ] {
        let _ = writeln!(pack, "pub const {name}: [u8; 32] = {digest:?};");
    }
    let _ = writeln!(pack, "pub static BOOT_SHARDS: &[DenseShard] = &[");
    for shard in &boot_shards {
        let block = &shard.blocks[0];
        let source_start = rom_start + (block.start_va - va_start);
        let source_end = source_start + u32::try_from(block.words.len() * 4).unwrap();
        let source_sha256 = fn64_discover::dense_aot_pack::dense_aot_shard_source_identity(
            &rom.sha256,
            "boot",
            source_start,
            source_end,
            block.start_va,
            block.start_va + u32::try_from(block.words.len() * 4).unwrap(),
            &rom.bytes[source_start as usize..source_end as usize],
        );
        let code_sha256: [u8; 32] =
            Sha256::digest(&rom.bytes[source_start as usize..source_end as usize]).into();
        let _ = writeln!(
            pack,
            "    DenseShard {{ bank_id: {:#018X}, va_start: {:#010X}, byte_len: {:#X}, source_sha256: {source_sha256:?}, code_sha256: {code_sha256:?} }},",
            shard.bank_id,
            block.start_va,
            block.words.len() * 4
        );
    }
    let _ = writeln!(pack, "];");
    let _ = writeln!(pack, "pub static RESIDENT_TAIL_SHARDS: &[DenseShard] = &[");
    for shard in &resident_tail_shards {
        let block = &shard.blocks[0];
        let source_start = rom_start + (block.start_va - va_start);
        let source_end = source_start + u32::try_from(block.words.len() * 4).unwrap();
        let bytes = &rom.bytes[source_start as usize..source_end as usize];
        let source_sha256 = fn64_discover::dense_aot_pack::dense_aot_shard_source_identity(
            &rom.sha256,
            "resident_tail",
            source_start,
            source_end,
            block.start_va,
            block.start_va + u32::try_from(block.words.len() * 4).unwrap(),
            bytes,
        );
        let code_sha256: [u8; 32] = Sha256::digest(bytes).into();
        let _ = writeln!(
            pack,
            "    DenseShard {{ bank_id: {:#018X}, va_start: {:#010X}, byte_len: {:#X}, source_sha256: {source_sha256:?}, code_sha256: {code_sha256:?} }},",
            shard.bank_id,
            block.start_va,
            block.words.len() * 4
        );
    }
    let _ = writeln!(pack, "];");
    // THE DISPATCH INVENTORY AND THE GENERATION'S SHARD LIST MUST BE THE SAME
    // LIST, and this is the check that they are.
    //
    // `RESIDENT_TAIL_SHARDS` is both: `block_program.rs:226-233` zips the
    // compiled artifacts against the generations' shard lists concatenated, and
    // `block_program.rs:249` decides an artifact's role by testing its index
    // against `RESIDENT_TAIL_SHARDS.len()`. So a generation list that is not
    // exactly the dispatch list puts that zip out of phase at the overlay
    // boundary -- observed as a bank-id mismatch, `assert_eq!` at
    // `block_program.rs:236` reporting 12916223148186077599 != 5445896104744520571.
    //
    // Two independent rules therefore have to produce the same count:
    //
    //   the SHARD GENERATOR  tiles [split, tail_end) at 64 KiB
    //     (`examples/revenge-block-shards/build.rs::resident_shard_counts`)
    //   the RUNTIME CATALOG  takes byte_len.div_ceil(SHARD_BYTES) over the
    //     clamped image (`runtime_generation_catalog.rs:123-126`)
    //
    // They disagreed on the first non-WM2000 title, by exactly one shard,
    // because the generator tiled to the END OF THE BOOT COPY while the library
    // clamps to the overlay invalidation union. WM2000's union runs past the
    // boot copy so the clamp is a no-op and the two agreed by luck of geometry;
    // Revenge's stops 21,600 bytes short, giving 8 against the library's 7.
    // The generator now clamps too. This assert is what makes that agreement
    // enforced rather than assumed -- if either rule drifts again it fails
    // here, at build time, naming both numbers, instead of surfacing as an
    // opaque bank-id mismatch after a 90-second link.
    assert_eq!(
        resident_tail_generation_shard_count,
        resident_tail_shards.len(),
        "the resident-tail dispatch inventory ({} shards, tiled by the shard generator) and \
         the resident-tail generation ({resident_tail_generation_shard_count} shards, \
         div_ceil over the clamped image [{resident_image_start:#010X},\
         {resident_tail_image_end:#010X})) must be the same list; \
         `block_program.rs` zips them positionally",
        resident_tail_shards.len()
    );
    let _ = writeln!(
        pack,
        "pub struct OverlayGeneration {{ pub id: u64, pub image_start: u32, pub image_end: u32, pub invalidation_start: u32, pub invalidation_end: u32, pub sha256: [u8; 32], pub shards: &'static [DenseShard] }}"
    );
    // `image_end` is the CLAMPED end. This is the record the runtime turns into
    // a `PrecompiledGeneration`, and `generation/mod.rs:100` rejects
    // `invalidation_end < image_end` as `InvalidationDoesNotContainImage`. On
    // WM2000 the unclamped value satisfied that by luck of geometry -- its
    // overlays run 0x71660 bytes past the boot bank. On Revenge the unclamped
    // 0x80100400 against an invalidation end of 0x800fafa0 is exactly the
    // rejected shape, so emitting it would move the failure from build time to
    // catalog-construction time rather than fixing it. The 21,600-byte span
    // between the two is measured data, not code (see the note at the clamp),
    // and stays immutable resident territory owned by the pre-split prefix
    // rule, which is what the library calls the correct ownership.
    let _ = writeln!(
        pack,
        "pub static RESIDENT_TAIL_GENERATION: OverlayGeneration = OverlayGeneration {{ id: {resident_tail_generation_id:#018X}, image_start: {resident_image_start:#010X}, image_end: {resident_tail_image_end:#010X}, invalidation_start: {invalidation_start:#010X}, invalidation_end: {invalidation_end:#010X}, sha256: {resident_tail_sha256:?}, shards: RESIDENT_TAIL_SHARDS }};"
    );
    for (index, ((name, recipe), generation)) in overlay_names
        .iter()
        .zip(&overlay_recipes)
        .zip(&overlay_dense_pack.generations)
        .enumerate()
    {
        let source_rom_end =
            recipe.rom_start + fn64_discover::overlay_recipe::generation_source_span(recipe);
        let source = &rom.bytes[recipe.rom_start as usize..source_rom_end as usize];
        let _ = writeln!(
            pack,
            "pub static OVERLAY_{index}_SHARDS: &[DenseShard] = &["
        );
        for (shard_index, bytes) in source.chunks(BOOT_SHARD_BYTES).enumerate() {
            let words = bytes
                .chunks_exact(4)
                .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                .collect::<Vec<_>>();
            let shard_va = recipe.load_start
                + u32::try_from(shard_index * BOOT_SHARD_BYTES)
                    .expect("overlay shard VA offset fits u32");
            let bank_id = fn64_discover::dense_aot_pack::dense_aot_artifact_bank_id(
                &rom.sha256,
                name,
                shard_va,
                &words,
            );
            let manifest_shard = &generation.shards[shard_index];
            assert_eq!(
                (manifest_shard.va_start, manifest_shard.va_end),
                (shard_va, shard_va + u32::try_from(bytes.len()).unwrap())
            );
            let source_sha256 = fn64_discover::dense_aot_pack::dense_aot_shard_source_identity(
                &rom.sha256,
                name,
                manifest_shard.source_rom_start,
                manifest_shard.source_rom_end,
                manifest_shard.va_start,
                manifest_shard.va_end,
                bytes,
            );
            let code_sha256: [u8; 32] = Sha256::digest(bytes).into();
            let _ = writeln!(
                pack,
                "    DenseShard {{ bank_id: {bank_id:#018X}, va_start: {shard_va:#010X}, byte_len: {:#X}, source_sha256: {source_sha256:?}, code_sha256: {code_sha256:?} }},",
                bytes.len()
            );
        }
        let _ = writeln!(pack, "];");
    }
    let _ = writeln!(
        pack,
        "pub static OVERLAY_GENERATIONS: &[OverlayGeneration] = &["
    );
    for (index, (recipe, generation)) in overlay_recipes
        .iter()
        .zip(&overlay_dense_pack.generations)
        .enumerate()
    {
        // The generation is the TEXT extent only: the data section past it is
        // mutable, and digesting it made a correct program invalidate its own
        // generation. Generations may now end mid-shard, so this is exact.
        // Same span as image_end below and as the shard source above, all
        // from `generation_source_span`.
        let digest_rom_end =
            recipe.rom_start + fn64_discover::overlay_recipe::generation_source_span(recipe);
        let digest: Vec<u8> =
            Sha256::digest(&rom.bytes[recipe.rom_start as usize..digest_rom_end as usize]).to_vec();
        let _full_image_digest = recipe
            .loaded_sha256
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16)
                    .expect("recovered overlay SHA-256 is hexadecimal")
            })
            .collect::<Vec<_>>();
        let _ = writeln!(
            pack,
            "    OverlayGeneration {{ id: {:#018X}, image_start: {:#010X}, image_end: {:#010X}, invalidation_start: {:#010X}, invalidation_end: {:#010X}, sha256: {digest:?}, shards: OVERLAY_{index}_SHARDS }},",
            generation.bank_id,
            recipe.load_start,
            recipe.load_start + fn64_discover::overlay_recipe::generation_source_span(recipe),
            recipe.load_start,
            recipe.bss_end,
        );
    }
    let _ = writeln!(pack, "];");
    let _ = writeln!(
        pack,
        // A plain struct of geometry plus a plain `words()` method that reads
        // the user's ROM. No `Deref`, no lazily-materializing static: on the
        // release-critical "no copyrighted content" path the absence of
        // embedded words should be auditable by reading this, and a clever
        // container that produces ROM words on first deref is not.
        //
        // `words()` recovers from the ROM on each call. The only caller that
        // runs per-execution is `verify_precompiled_words` on the exception
        // vector, over 4 words; the rest are startup. Callers keep the same
        // `&[u32]`-shaped argument, so `verify_precompiled_words` and
        // `CodeBank::new` are unchanged.
        "pub struct ExternalExecutableImage {{ pub image_id: &'static str, pub generation: u64, pub bank_id: u64, pub va_start: u32, pub va_end: u32, pub sha256_hex: &'static str, pub sha256: [u8; 32], pub rom_start: u32, pub rom_end: u32 }}\n\
         impl ExternalExecutableImage {{\n\
         \x20   /// This image's instruction words, read from the user's ROM at the\n\
         \x20   /// offsets recorded at build time. Nothing is embedded; the caller\n\
         \x20   /// verifies the result against `self.sha256`, exactly as it did\n\
         \x20   /// when these words were a baked array.\n\
         \x20   pub fn words(&self) -> Vec<u32> {{\n\
         \x20       fn64_recomp_rs::shard_words(self.rom_start, self.rom_end)\n\
         \x20           .unwrap_or_else(|error| panic!(\"external executable image {{:?}} cannot recover its words from the user's ROM: {{error}}\", self.image_id))\n\
         \x20   }}\n\
         \x20   /// How many instruction words this image has, WITHOUT reading the ROM.\n\
         \x20   ///\n\
         \x20   /// A count is pure geometry, so it must not require a published ROM.\n\
         \x20   /// The startup banner prints this before `load_rom` runs; calling\n\
         \x20   /// `words()` there panicked, because recovery needs an image that\n\
         \x20   /// does not exist yet. Length is derivable from the extent alone.\n\
         \x20   pub fn word_count(&self) -> usize {{\n\
         \x20       ((self.rom_end - self.rom_start) / 4) as usize\n\
         \x20   }}\n\
         }}"
    );
    // Captured exception-vector images are ROM content too, so they are
    // located in the ROM and emitted as geometry rather than as literal words.
    // The audit established that WM2000's one image (4 words at VA 0x80000180)
    // appears verbatim at ROM 0x37380; this searches rather than hardcodes so
    // the failure is loud on any ROM or route where it does not hold.
    //
    // The search must find EXACTLY ONE occurrence. A unique match is what makes
    // the offset an identity; several matches would make the choice arbitrary,
    // and this is small enough (16 bytes) that coincidence is a real risk worth
    // rejecting rather than tie-breaking.
    let mut external_image_rom_offsets: Vec<u32> = Vec::with_capacity(vector_banks.len());
    for vector_bank in &vector_banks {
        let needle: Vec<u8> = vector_bank.blocks[0]
            .words
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect();
        let mut found: Option<usize> = None;
        let mut occurrences = 0usize;
        for offset in (0..rom.bytes.len().saturating_sub(needle.len())).step_by(4) {
            if rom.bytes[offset..offset + needle.len()] == needle[..] {
                occurrences += 1;
                if found.is_none() {
                    found = Some(offset);
                }
                if occurrences > 1 {
                    break;
                }
            }
        }
        let offset = match (found, occurrences) {
            (Some(offset), 1) => offset,
            (_, count) => panic!(
                "captured exception image {:?} (generation {}) has {count} word-aligned \
                 occurrences in the ROM; exactly one is required for it to be emitted as \
                 geometry instead of embedded words",
                vector_bank.bank, count
            ),
        };
        external_image_rom_offsets.push(u32::try_from(offset).expect("ROM offset exceeds u32"));
    }
    let _ = writeln!(
        pack,
        "pub static EXTERNAL_EXECUTABLE_IMAGES: &[ExternalExecutableImage] = &["
    );
    for (index, (image, vector_bank)) in external_catalog.iter().zip(&vector_banks).enumerate() {
        let digest_bytes = image
            .capture
            .sha256
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16)
                    .expect("validated executable-image SHA-256 is hexadecimal")
            })
            .collect::<Vec<_>>();
        let block = &vector_bank.blocks[0];
        let va_end = block.start_va + block.words.len() as u32 * 4;
        let rom_start = external_image_rom_offsets[index];
        let rom_end = rom_start + block.words.len() as u32 * 4;
        let _ = writeln!(
            pack,
            "    ExternalExecutableImage {{ image_id: {:?}, generation: {}, bank_id: {:#018X}, va_start: {:#010X}, va_end: {va_end:#010X}, sha256_hex: {:?}, sha256: {digest_bytes:?}, rom_start: {rom_start:#010X}, rom_end: {rom_end:#010X} }},",
            image.capture.image_id,
            image.capture.generation,
            vector_bank.bank_id,
            block.start_va,
            image.capture.sha256,
        );
    }
    let _ = writeln!(pack, "];");

    let mut dispatch_identity = Sha256::new();
    dispatch_identity.update(b"fn64:wm2000-production-dispatch-input:v1:");
    dispatch_identity.update((dispatch_source.len() as u64).to_be_bytes());
    dispatch_identity.update(&dispatch_source);
    dispatch_identity.update(external_runner_source_sha256);
    dispatch_identity.update((pack.len() as u64).to_be_bytes());
    dispatch_identity.update(pack.as_bytes());
    let dispatch_source_sha256: [u8; 32] = dispatch_identity.finalize().into();
    let _ = writeln!(
        pack,
        "pub const DISPATCH_SOURCE_SHA256: [u8; 32] = {dispatch_source_sha256:?};"
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    write_if_changed(&out_dir.join("runner.rs"), &runner);
    write_if_changed(&out_dir.join("pack.rs"), &pack);
    println!(
        "cargo:warning=wm2000-block-boot: packed {} static-prefix + {} resident-tail + {} overlay dense-AOT shards / {} resident words from mechanical ROM discovery; {} captured exception images",
        boot_shards.len(),
        resident_tail_shards.len(),
        overlay_dense_pack.generations.iter().map(|generation| generation.shards.len()).sum::<usize>(),
        dense_words.len(),
        external_catalog.len(),
    );
    for image in &external_catalog {
        println!(
            "cargo:warning=wm2000-block-boot: exception image {} generation {} contributes {} words at {:#010x} ({})",
            image.capture.image_id,
            image.capture.generation,
            image.capture.words.len(),
            image.capture.va_start,
            image.capture.sha256,
        );
    }
}
