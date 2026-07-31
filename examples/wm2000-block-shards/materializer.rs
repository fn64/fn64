//! Dependency-free consumer for a privately prepared WM shard source tree.
//!
//! The selected package sidecar contains identities only. The private tree's
//! generated Rust is ROM-derived game content, including instruction words,
//! so it must never enter git, logs, or receipts. This consumer is not
//! authority: the cold verifier must independently measure the root manifest,
//! every sidecar/source file, and the selected compiler artifact.

use std::fmt;
use std::fs;
use std::path::Path;

pub const PREPARED_ROOT_ENV: &str = "FN64_WM_PREPARED_SHARD_ROOT";
pub const ARTIFACT_SCHEMA_V1: &str = "fn64.wm-prepared-shard-artifact.v1";
pub const IDENTITY_NAME: &str = "identity.v1";
pub const UPDATE_MARKER_NAME: &str = ".update.v2";

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
pub struct ArtifactIdentity {
    runner_sha256: [u8; 32],
    metadata_sha256: [u8; 32],
}

#[derive(Debug)]
pub struct MaterializeError(String);

impl fmt::Display for MaterializeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MaterializeError {}

fn error(message: impl Into<String>) -> MaterializeError {
    MaterializeError(message.into())
}

pub fn emit_cargo_directives(root: &Path, package: &str) {
    println!("cargo:rerun-if-env-changed={PREPARED_ROOT_ENV}");
    println!(
        "cargo:rerun-if-changed={}",
        root.join(package).join(IDENTITY_NAME).display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join(package).join("runner.rs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join(package).join("metadata.rs").display()
    );
}

pub fn materialize_package(
    root: &Path,
    package: &str,
    out_dir: &Path,
) -> Result<(), MaterializeError> {
    if PACKAGES.binary_search(&package).is_err() {
        return Err(error(format!("unknown WM shard package {package}")));
    }
    require_directory(root, "prepared root")?;
    require_stable_projection(root)?;
    let package_dir = root.join(package);
    require_directory(&package_dir, "prepared package directory")?;
    let identity_bytes = read_regular(&package_dir.join(IDENTITY_NAME), "prepared identity")?;
    let expected = parse_identity(&identity_bytes, package)?;
    let runner = read_regular(&package_dir.join("runner.rs"), "prepared runner")?;
    let metadata = read_regular(&package_dir.join("metadata.rs"), "prepared metadata")?;
    verify_digest("runner.rs", &runner, expected.runner_sha256)?;
    verify_digest("metadata.rs", &metadata, expected.metadata_sha256)?;
    require_stable_projection(root)?;
    require_directory(out_dir, "Cargo OUT_DIR")?;
    write_if_changed(&out_dir.join("runner.rs"), &runner)?;
    write_if_changed(&out_dir.join("metadata.rs"), &metadata)?;
    require_stable_projection(root)?;
    Ok(())
}

fn require_stable_projection(root: &Path) -> Result<(), MaterializeError> {
    match fs::symlink_metadata(root.join(UPDATE_MARKER_NAME)) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(error(
            "prepared artifact projection is being updated; concurrent Cargo is forbidden",
        )),
        Err(source) => Err(error(format!("inspect prepared update marker: {source}"))),
    }
}

pub fn parse_identity(
    bytes: &[u8],
    expected_package: &str,
) -> Result<ArtifactIdentity, MaterializeError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("prepared identity is not UTF-8: {source}")))?;
    if text.contains('\r') || !text.ends_with('\n') {
        return Err(error(
            "prepared identity must use canonical LF-terminated lines",
        ));
    }
    let mut lines = text.lines();
    expect_line(&mut lines, "schema", ARTIFACT_SCHEMA_V1)?;
    expect_line(&mut lines, "package", expected_package)?;
    let runner_sha256 = digest_line(&mut lines, "runner_sha256")?;
    let metadata_sha256 = digest_line(&mut lines, "metadata_sha256")?;
    if let Some(extra) = lines.next() {
        return Err(error(format!(
            "prepared identity has trailing line: {extra}"
        )));
    }
    Ok(ArtifactIdentity {
        runner_sha256,
        metadata_sha256,
    })
}

fn expect_line<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    field: &str,
    expected: &str,
) -> Result<(), MaterializeError> {
    let line = lines
        .next()
        .ok_or_else(|| error(format!("prepared identity omits {field}")))?;
    if line != format!("{field} {expected}") {
        return Err(error(format!(
            "prepared identity has invalid {field}: {line}"
        )));
    }
    Ok(())
}

fn digest_line<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    field: &'static str,
) -> Result<[u8; 32], MaterializeError> {
    let line = lines
        .next()
        .ok_or_else(|| error(format!("prepared identity omits {field}")))?;
    let (observed_field, value) = line
        .split_once(' ')
        .ok_or_else(|| error(format!("prepared identity has malformed {field}")))?;
    if observed_field != field || value.contains(' ') {
        return Err(error(format!("prepared identity has noncanonical {field}")));
    }
    parse_digest(value, field)
}

fn parse_digest(value: &str, field: &str) -> Result<[u8; 32], MaterializeError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(error(format!("prepared {field} is not lowercase SHA-256")));
    }
    let mut digest = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        digest[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    if digest == [0; 32] {
        return Err(error(format!("prepared {field} is zero")));
    }
    Ok(digest)
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("validated lowercase hex"),
    }
}

fn verify_digest(label: &str, bytes: &[u8], expected: [u8; 32]) -> Result<(), MaterializeError> {
    let observed = sha256(bytes);
    if observed != expected {
        return Err(error(format!(
            "prepared {label} SHA-256 mismatch: expected={}, observed={}",
            hex(expected),
            hex(observed)
        )));
    }
    Ok(())
}

fn require_directory(path: &Path, label: &str) -> Result<(), MaterializeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect {label} {}: {source}", path.display())))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error(format!("{label} must be a non-symlink directory")));
    }
    Ok(())
}

fn read_regular(path: &Path, label: &str) -> Result<Vec<u8>, MaterializeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect {label} {}: {source}", path.display())))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(format!("{label} must be a regular non-symlink file")));
    }
    fs::read(path).map_err(|source| error(format!("read {label} {}: {source}", path.display())))
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), MaterializeError> {
    if fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    fs::write(path, bytes).map_err(|source| {
        error(format!(
            "write materialized source {}: {source}",
            path.display()
        ))
    })
}

fn hex(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

// FIPS 180-4 SHA-256. Kept here so the eventual build script has no Cargo
// dependencies and cannot regain a transitive edge to discovery or codegen.
fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    let mut state = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in padded.chunks_exact(64) {
        let mut words = [0u32; 64];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(bytes.try_into().expect("four-byte word"));
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut output = [0u8; 32];
    for (chunk, value) in output.chunks_exact_mut(4).zip(state) {
        chunk.copy_from_slice(&value.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        out: PathBuf,
        runners: BTreeMap<&'static str, Vec<u8>>,
        metadata: BTreeMap<&'static str, Vec<u8>>,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "fn64-prepared-shard-test-{}-{nonce}",
                std::process::id()
            ));
            let out = root.join("out");
            fs::create_dir(&root).unwrap();
            fs::create_dir(&out).unwrap();
            let mut fixture = Self {
                root,
                out,
                runners: BTreeMap::new(),
                metadata: BTreeMap::new(),
            };
            for package in PACKAGES {
                let package_dir = fixture.root.join(package);
                fs::create_dir(&package_dir).unwrap();
                let runner =
                    format!("pub fn synthetic_{}() {{}}\n", package.replace('-', "_")).into_bytes();
                let metadata =
                    format!("pub const SYNTHETIC_PACKAGE: &str = {package:?};\n").into_bytes();
                fs::write(package_dir.join("runner.rs"), &runner).unwrap();
                fs::write(package_dir.join("metadata.rs"), &metadata).unwrap();
                fixture.runners.insert(package, runner);
                fixture.metadata.insert(package, metadata);
                fixture.write_identity(package, None);
            }
            fixture
        }

        fn identity(&self, package: &str) -> String {
            format!(
                "schema {ARTIFACT_SCHEMA_V1}\npackage {package}\nrunner_sha256 {}\nmetadata_sha256 {}\n",
                hex(sha256(&self.runners[package])),
                hex(sha256(&self.metadata[package]))
            )
        }

        fn write_identity(&self, package: &str, replacement: Option<String>) {
            fs::write(
                self.root.join(package).join(IDENTITY_NAME),
                replacement.unwrap_or_else(|| self.identity(package)),
            )
            .unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn sha256_matches_fips_vector() {
        assert_eq!(
            hex(sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn synthetic_tree_without_root_manifest_materializes_one_exact_package() {
        let fixture = Fixture::new();
        let package = PACKAGES[24];
        materialize_package(&fixture.root, package, &fixture.out).unwrap();
        assert_eq!(
            fs::read(fixture.out.join("runner.rs")).unwrap(),
            fixture.runners[package]
        );
        assert_eq!(
            fs::read(fixture.out.join("metadata.rs")).unwrap(),
            fixture.metadata[package]
        );
    }

    #[test]
    fn unknown_package_and_extra_identity_line_fail_closed() {
        let fixture = Fixture::new();
        assert!(materialize_package(&fixture.root, "wm2000-block-shard-99", &fixture.out).is_err());
        let package = PACKAGES[0];
        fixture.write_identity(
            package,
            Some(format!("{}extra rejected\n", fixture.identity(package))),
        );
        assert!(materialize_package(&fixture.root, package, &fixture.out).is_err());
    }

    #[test]
    fn missing_artifact_and_digest_mismatch_fail_closed() {
        let fixture = Fixture::new();
        let package = PACKAGES[0];
        fs::remove_file(fixture.root.join(package).join("runner.rs")).unwrap();
        assert!(materialize_package(&fixture.root, package, &fixture.out).is_err());
        fs::write(fixture.root.join(package).join("runner.rs"), b"changed").unwrap();
        assert!(materialize_package(&fixture.root, package, &fixture.out).is_err());
    }

    #[test]
    fn in_progress_projection_update_fails_closed() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join(UPDATE_MARKER_NAME), b"synthetic\n").unwrap();
        assert!(materialize_package(&fixture.root, PACKAGES[0], &fixture.out).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_artifact_fails_closed() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let package = PACKAGES[0];
        let runner = fixture.root.join(package).join("runner.rs");
        fs::remove_file(&runner).unwrap();
        symlink(fixture.root.join(package).join("metadata.rs"), runner).unwrap();
        assert!(materialize_package(&fixture.root, package, &fixture.out).is_err());
    }

    #[test]
    fn malformed_schema_package_and_zero_digest_fail_closed() {
        let fixture = Fixture::new();
        let package = PACKAGES[0];
        for malformed in [
            fixture.identity(package).replacen(
                ARTIFACT_SCHEMA_V1,
                "fn64.wm-prepared-shard-artifact.v0",
                1,
            ),
            fixture.identity(package).replacen(package, PACKAGES[1], 1),
            fixture.identity(package).replacen(
                &hex(sha256(&fixture.runners[package])),
                &"00".repeat(32),
                1,
            ),
        ] {
            fixture.write_identity(package, Some(malformed));
            assert!(materialize_package(&fixture.root, package, &fixture.out).is_err());
        }
    }
}
