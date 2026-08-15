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
/// Directory CONTAINING the per-title shard directories. Absolute. Unset =
/// the in-repo `examples/`, so the default build is unchanged.
const SHARD_ROOT_ENV: &str = "FN64_SHARD_ROOT";

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

    // Stage the shard inventory into OUT_DIR so `SHARD_INVENTORY`'s `include!`
    // does NOT reach into a sibling directory at compile time.
    //
    // That reach is what made the game harnesses inextricable: a core crate
    // that `include!`s `../../../../examples/<title>/shard_inventory.in`
    // cannot compile once those packages live in another repository. Resolving
    // the file here — absolute, overridable, copied into OUT_DIR — moves the
    // dependency from "a path baked into fn64's source tree" to "an input this
    // build was given".
    //
    // `FN64_SHARD_ROOT` names the directory CONTAINING the per-title shard
    // directories. Unset, it is the in-repo `examples/`, so today's build is
    // reproduced byte-for-byte and nothing downstream changes yet.
    println!("cargo:rerun-if-env-changed={SHARD_ROOT_ENV}");
    let shard_root = match env::var_os(SHARD_ROOT_ENV) {
        Some(root) => PathBuf::from(root),
        None => {
            let manifest = PathBuf::from(
                env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR"),
            );
            manifest
                .parent()
                .and_then(|crates| crates.parent())
                .expect("crates/<pkg> must have a repository root")
                .join("examples")
        }
    };
    assert!(
        shard_root.is_absolute(),
        "{SHARD_ROOT_ENV} must be an absolute path, got {shard_root:?}"
    );
    let inventory = shard_root.join(&shard_dir).join("shard_inventory.in");
    println!("cargo:rerun-if-changed={}", inventory.display());
    let staged = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"))
        .join("shard_inventory.in");
    // A MISSING inventory is the ordinary case now, not an error: the game
    // packages live in their own repository, so a plain `git clone` of fn64
    // has no `examples/` at all. Requiring the file would make fn64
    // unbuildable standalone -- exactly the coupling this extraction removed,
    // reintroduced through the back door. CI proved the point by failing here.
    //
    // An empty inventory yields SHARD_COUNT == 0, so the generated-runner
    // build paths that need shards fail loudly at RUN time with a real message
    // instead of blocking compilation for everyone. Set FN64_SHARD_ROOT to
    // build against a real game checkout.
    let bytes = fs::read(&inventory).unwrap_or_else(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            println!(
                "cargo:warning=fn64-boot-harness: no shard inventory at {} -- \
                 building with an EMPTY shard set. Set FN64_SHARD_ROOT to a \
                 directory containing the game packages to build against one.",
                inventory.display()
            );
            return b"[]".to_vec();
        }
        panic!("read shard inventory {}: {error}", inventory.display());
    });
    fs::write(&staged, &bytes).unwrap_or_else(|error| {
        panic!("stage shard inventory to {}: {error}", staged.display());
    });
}
