//! `fn64-recomp`: the static-recompiler seam, per `docs/DECOUPLING.md`
//! ("Recompiler seam — `fn64-recomp`").
//!
//! ## What this crate is
//!
//! `Recompiler` is the ONE boundary a caller uses to turn ROM bytes + typed
//! symbol/patch metadata into generated C: `recompile` (CPU code) and
//! `recompile_rsp` (RSP microcode), plus `abi_version` so a mismatch against
//! `fn64-abi` fails loudly at plug-in time instead of at link time.
//!
//! `RecompConfig`/`RspConfig` ([`config`]) are OUR typed representation —
//! sections, functions, stubs, patches, hooks — never hand-serialized TOML
//! strings. [`load`] reads N64Recomp-format configs (`oot.toml` + symbol
//! dump) into that typed representation; it is the READ side of the format.
//!
//! ## What this crate is now
//!
//! `fn64-recomp-rs` (the from-scratch, all-Rust VR4300 recompiler,
//! `docs/DECOUPLING.md` step 5) implements the [`Recompiler`] trait for CPU
//! code and is the whole-ROM recompile path in tree; see
//! `docs/RECOMP-RS-COVERAGE.md`. This crate now provides only the shared
//! seam types (the [`Recompiler`] trait, [`RecompConfig`]/[`RspConfig`],
//! [`AbiVersion`], [`RecompOutput`]) and the [`load`] reader those consumers
//! share.
//!
//! The former `n64recomp` adapter — which serialized a `RecompConfig` to
//! N64Recomp/RSPRecomp TOML and shelled out to the pinned fork's binaries —
//! has been retired: it had no in-tree consumer once `fn64-recomp-rs` became
//! the recompiler. The pre-generated-C CI oracle lane (compiled by
//! `fn64-boot-harness`/`fn64-shell` build scripts) is unaffected.
#![forbid(unsafe_code)]

pub mod config;
pub mod load;

use std::fmt;

pub use config::{Function, Hook, InstructionPatch, Patches, RecompConfig, RspConfig, Section};
pub use load::{load_config, load_symbols, LoadError};

/// The fn64 ABI version a recompiler's generated C targets, checked against
/// `fn64-abi` at plug-in time (`docs/DECOUPLING.md`: "any impl must emit
/// code that links against it"). A plain `(major, minor)` pair — this crate
/// doesn't depend on `fn64-abi` itself (keeping the trait side of this seam
/// dependency-free, matching `fn64-render`'s precedent of not depending on
/// `fn64-runtime`); the caller is the one who compares this against
/// whatever `fn64-abi` constant it links.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AbiVersion {
    pub major: u32,
    pub minor: u32,
}

impl AbiVersion {
    pub const fn new(major: u32, minor: u32) -> Self {
        AbiVersion { major, minor }
    }
}

impl fmt::Display for AbiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// One recompile's output: the generated C sources plus which function/
/// microcode names actually got recompiled (vs. stubbed/skipped), so a
/// caller can verify coverage without re-parsing the emitted C itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecompOutput {
    /// Generated source files, keyed by the path the recompiler wrote them
    /// at (relative to the config's `output_func_path`/`output_file_path`).
    pub generated_files: Vec<(std::path::PathBuf, String)>,
    /// Names the recompiler actually emitted a real body for (i.e. NOT in
    /// `patches.stubs`/`ignored` and not one of N64Recomp's own
    /// reimplemented/ignored/renamed built-ins).
    pub recompiled_functions: Vec<String>,
}

/// Everything that can go wrong at this seam. Every variant is loud/named,
/// mirroring `fn64_render::RenderError`'s "no silent failure" rule — there
/// is no `RecompError::Other(String)` catch-all.
#[derive(Debug)]
pub enum RecompError {
    /// The typed config could not be translated into the target
    /// recompiler's own format (e.g. an adapter-specific constraint the
    /// typed config doesn't uphold).
    InvalidConfig(String),
    /// Shelling out to the underlying recompiler binary failed to launch at
    /// all (binary missing, not executable, etc).
    Launch { binary: String, reason: String },
    /// The underlying recompiler binary ran and exited non-zero, or emitted
    /// diagnostics indicating failure. Carries its captured output so the
    /// caller can surface exactly what N64Recomp/RSPRecomp said.
    RecompilerFailed { binary: String, output: String },
    /// The recompile step reported success but the expected generated
    /// output was not found on disk afterward.
    MissingOutput(std::path::PathBuf),
    /// `abi_version()` from the concrete `Recompiler` didn't match what the
    /// caller expected — the fail-loudly-at-plug-in-time case the ABI
    /// version field exists for.
    AbiMismatch {
        expected: AbiVersion,
        actual: AbiVersion,
    },
}

impl fmt::Display for RecompError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecompError::InvalidConfig(reason) => write!(f, "invalid recompile config: {reason}"),
            RecompError::Launch { binary, reason } => {
                write!(f, "failed to launch {binary}: {reason}")
            }
            RecompError::RecompilerFailed { binary, output } => {
                write!(f, "{binary} failed:\n{output}")
            }
            RecompError::MissingOutput(path) => {
                write!(f, "expected recompiler output missing: {}", path.display())
            }
            RecompError::AbiMismatch { expected, actual } => write!(
                f,
                "recompiler targets ABI {actual}, caller expected {expected}"
            ),
        }
    }
}

impl std::error::Error for RecompError {}

/// A static recompiler: symbol/patch metadata + ROM in, generated C + a
/// recompiled-function manifest out. Per `docs/DECOUPLING.md`, the fn64 ABI
/// (`fn64-abi`) is the fixed target; any impl must emit code that links
/// against it, which is exactly what `abi_version()` lets a caller verify
/// before ever invoking `recompile`.
pub trait Recompiler {
    /// Recompile CPU code per `cfg` (the `[input]`/symbol-table/`[patches]`
    /// shape — see [`RecompConfig`]).
    fn recompile(&self, cfg: &RecompConfig) -> Result<RecompOutput, RecompError>;

    /// Recompile one RSP microcode program per `cfg` (see [`RspConfig`]).
    fn recompile_rsp(&self, cfg: &RspConfig) -> Result<RecompOutput, RecompError>;

    /// The ABI version this recompiler's generated C targets.
    fn abi_version(&self) -> AbiVersion;
}
