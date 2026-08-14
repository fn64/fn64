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

// The default preserves WM2000's current behavior byte-for-byte when the
// selector is unset, so the existing byte-identity gate is the regression
// proof for every change this selector makes possible.
const DEFAULT_WM_SHARD_DIR: &str = "wm2000-block-shards";
const WM_SHARD_TITLE_ENV: &str = "FN64_WM_SHARD_TITLE";

/// Validate the selector is a bare directory name: no path separators, no
/// `..` traversal, non-empty. `SHARD_INVENTORY`'s `include!` and every
/// `wm2000-block-shards` literal this feeds resolve it relative to
/// `examples/`, so anything else would either escape that directory or
/// silently fail to `include!` at all -- and an invalid value must fail the
/// build loudly rather than fall back to the default, the same way a typo'd
/// env value must never be read as "unset" elsewhere in this project.
fn validate_shard_title(title: &str) {
    assert!(!title.is_empty(), "{WM_SHARD_TITLE_ENV} must not be empty");
    assert!(
        title != "." && title != "..",
        "{WM_SHARD_TITLE_ENV} must not be a directory-traversal segment: {title:?}"
    );
    assert!(
        !title.contains('/') && !title.contains('\\'),
        "{WM_SHARD_TITLE_ENV} must be a single path segment, got {title:?}"
    );
    assert!(
        title
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "{WM_SHARD_TITLE_ENV} must contain only ASCII alphanumerics, '-' and '_', got {title:?}"
    );
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

    println!("cargo:rerun-if-env-changed={WM_SHARD_TITLE_ENV}");
    let shard_dir = env::var(WM_SHARD_TITLE_ENV).unwrap_or_else(|_| DEFAULT_WM_SHARD_DIR.to_owned());
    validate_shard_title(&shard_dir);
    println!("cargo:rustc-env=FN64_WM_SHARD_DIR={shard_dir}");
}
