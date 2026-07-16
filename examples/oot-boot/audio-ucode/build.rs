use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let generated =
        manifest_dir.join("../../../../aki-recomp/games/OOTU/rsp-recomp/src/oot_aspmain.rs");
    assert!(
        generated.is_file(),
        "OoT audio adapter: generated aspMain module not found at {}",
        generated.display()
    );
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("oot_aspmain.rs");
    let source = std::fs::read_to_string(&generated)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", generated.display()));
    // The emitter writes a crate-level `#![allow(...)]`. This adapter includes
    // the generated file as a module, where an inner attribute is invalid; the
    // adapter crate carries the identical allow at its own root instead.
    let source = source
        .lines()
        .filter(|line| !line.starts_with("#![allow("))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&out, source)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", out.display()));
    println!("cargo:rerun-if-changed={}", generated.display());
}
