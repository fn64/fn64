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
//! strings. [`n64recomp`] is the one adapter that knows N64Recomp exists:
//! it translates a `RecompConfig`/`RspConfig` into N64Recomp's/RSPRecomp's
//! TOML shape and shells out to the pinned fork's binaries. **This is the
//! only crate that names N64Recomp** (`docs/DECOUPLING.md`'s explicit
//! requirement) — every other fn64 crate and the AKI tooling talk to
//! `Recompiler`, not to N64Recomp directly.
//!
//! ## What this crate is NOT (yet)
//!
//! This is the seam, not a working end-to-end recompile pipeline swap.
//! `aki_profile` (Python, `aki-recomp/tools/aki_profile`) keeps owning the
//! real WM2000/NW4E recompile loop today; this crate is the future home
//! that pipeline's shell-out logic moves behind, and the eventual swap
//! point for a from-scratch `fn64-recomp-rs` implementing the same
//! trait (`docs/DECOUPLING.md` step 5). No full-ROM recompile is attempted
//! here — see `n64recomp`'s golden serialization test for what IS proven:
//! a known `RecompConfig`/`RspConfig` round-trips to the exact TOML shape
//! real AKI-title configs already use.
#![forbid(unsafe_code)]

pub mod config;
pub mod load;
pub mod n64recomp;

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
