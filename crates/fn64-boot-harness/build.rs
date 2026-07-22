use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn main() {
    let cargo = PathBuf::from(env::var_os("CARGO").expect("Cargo must identify its executable"));
    assert!(
        cargo.is_absolute(),
        "Cargo executable path must be absolute"
    );
    let canonical =
        fs::canonicalize(&cargo).expect("resolve the Cargo executable used for this build");
    let bytes = fs::read(&canonical).expect("read the Cargo executable used for this build");
    let cargo_path = cargo
        .to_str()
        .expect("Cargo executable path must be UTF-8 for platform certification");
    let canonical_path = canonical
        .to_str()
        .expect("canonical Cargo executable path must be UTF-8 for platform certification");
    println!("cargo:rustc-env=FN64_BUILD_CARGO_PATH={cargo_path}");
    println!("cargo:rustc-env=FN64_BUILD_CARGO_CANONICAL_PATH={canonical_path}");
    println!(
        "cargo:rustc-env=FN64_BUILD_CARGO_SHA256={}",
        hex(&Sha256::digest(bytes))
    );
    println!(
        "cargo:rustc-env=FN64_BUILD_TARGET={}",
        env::var("TARGET").expect("Cargo must identify its target")
    );
}
