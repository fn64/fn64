//! Verifier-owned build authority for the repository's generated WM runner.
//!
//! The source attestation emitted by `fn64-recomp-rs` is intentionally not
//! authority: safe Rust cannot recover a function body's source from a
//! function pointer. This module closes that outer relation by owning one
//! frozen Cargo build, selecting the exact compiler artifact, launching only
//! that artifact's fixed identity mode, and retaining the result in a
//! move-only, non-serializable capability.


use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

mod types;
mod build;
mod runtime_series_a;
mod runtime_series_b;
mod runtime_series_c;
mod stage;

pub use types::*;
pub use build::*;
pub use runtime_series_a::*;
pub use runtime_series_b::*;
pub use runtime_series_c::*;
pub use stage::*;

pub const GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V2: &str =
    "fn64.generated-runner-build-identity.v2";
pub const VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V2: &str =
    "fn64.verified-generated-runner-build.v2";
pub const GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3: &str =
    "fn64.generated-runner-build-identity.v3";
pub const VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V3: &str =
    "fn64.verified-generated-runner-build.v3";
pub const VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V4: &str =
    "fn64.verified-generated-runner-build.v4";
pub const VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V5: &str =
    "fn64.verified-generated-runner-build.v5";
pub const GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-bootstrap-runtime-report.v1";
pub const GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-bootstrap-runtime-report=";
pub const GENERATED_RUNNER_BOOTSTRAP_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-bootstrap-audit-v1";
pub const GENERATED_RUNNER_BOOTSTRAP_RUNTIME_NONCE_ENV_V1: &str =
    "FN64_GENERATED_RUNNER_BOOTSTRAP_NONCE";
pub const VERIFIED_GENERATED_RUNNER_BOOTSTRAP_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-bootstrap-series.v1";
pub const GENERATED_RUNNER_CPU_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-cpu-runtime-report.v1";
pub const GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-cpu-runtime-report=";
pub const GENERATED_RUNNER_CPU_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-cpu-audit-v1";
pub const GENERATED_RUNNER_CPU_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_CPU_NONCE";
pub const VERIFIED_GENERATED_RUNNER_CPU_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-cpu-series.v1";
pub const GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-host-abi-runtime-report.v1";
pub const GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-host-abi-runtime-report=";
pub const GENERATED_RUNNER_HOST_ABI_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-host-abi-audit-v1";
pub const GENERATED_RUNNER_HOST_ABI_RUNTIME_NONCE_ENV_V1: &str =
    "FN64_GENERATED_RUNNER_HOST_ABI_NONCE";
pub const VERIFIED_GENERATED_RUNNER_HOST_ABI_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-host-abi-series.v1";
pub const GENERATED_RUNNER_PI_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-pi-runtime-report.v1";
pub const GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-pi-runtime-report=";
pub const GENERATED_RUNNER_PI_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-pi-audit-v1";
pub const GENERATED_RUNNER_PI_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_PI_NONCE";
pub const VERIFIED_GENERATED_RUNNER_PI_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-pi-series.v1";
pub const GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-rdp-renderer-runtime-report.v1";
pub const GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-rdp-renderer-runtime-report=";
pub const GENERATED_RUNNER_RDP_RENDERER_RUNTIME_ARGUMENT_V1: &str =
    "--fn64-run-rdp-renderer-audit-v1";
pub const GENERATED_RUNNER_RDP_RENDERER_RUNTIME_NONCE_ENV_V1: &str =
    "FN64_GENERATED_RUNNER_RDP_RENDERER_NONCE";
pub const VERIFIED_GENERATED_RUNNER_RDP_RENDERER_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-rdp-renderer-series.v1";
pub const GENERATED_RUNNER_RSP_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-rsp-runtime-report.v1";
pub const GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-rsp-runtime-report=";
pub const GENERATED_RUNNER_RSP_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-rsp-audit-v1";
pub const GENERATED_RUNNER_RSP_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_RSP_NONCE";
pub const VERIFIED_GENERATED_RUNNER_RSP_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-rsp-series.v1";
pub const VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-writer-audit-bundle.v1";
pub const GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1: &str = "fn64-generated-runner-build-identity=";
pub const GENERATED_RUNNER_BUILD_IDENTITY_ARGUMENT_V1: &str =
    "--fn64-emit-generated-runner-build-identity-v1";
pub const GENERATED_RUNNER_SI_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-si-runtime-report.v1";
pub const GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-si-runtime-report=";
pub const GENERATED_RUNNER_SI_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-si-audit-v1";
pub const GENERATED_RUNNER_SI_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_SI_NONCE";
pub const VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-si-series.v1";
pub const GENERATED_RUNNER_SP_RUNTIME_REPORT_SCHEMA_V1: &str =
    "fn64.generated-runner-sp-runtime-report.v1";
pub const GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1: &str =
    "fn64-generated-runner-sp-runtime-report=";
pub const GENERATED_RUNNER_SP_RUNTIME_ARGUMENT_V1: &str = "--fn64-run-sp-audit-v1";
pub const GENERATED_RUNNER_SP_RUNTIME_NONCE_ENV_V1: &str = "FN64_GENERATED_RUNNER_SP_NONCE";
pub const VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1: &str =
    "fn64.verified-generated-runner-sp-series.v1";

const PACKAGE: &str = "wm2000-block-boot";
const PRODUCER_PACKAGE: &str = "fn64-wm-prepared-shard-producer";
const SELECTED_BUILD_CARGO_JOBS_V5: u16 = 2;
const PREPARED_ROOT_ENV: &str = "FN64_WM_PREPARED_SHARD_ROOT";
const PREPARED_MANIFEST_NAME: &str = "manifest.v2";
const PREPARED_UPDATE_MARKER_NAME: &str = ".update.v2";
const PREPARED_SOURCE_MODE_INACTIVE_V1: &str = "legacy_with_prepared_candidate";
const PREPARED_SOURCE_MODE_CONSUMED_V1: &str = "prepared_consumed";
const PREPARED_PACKAGES: [&str; 35] = [
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
const SHARD_MANIFEST_DIRS: [&str; 35] = [
    "overlay0-shard00",
    "overlay0-shard01",
    "overlay0-shard02",
    "overlay1-shard00",
    "overlay2-shard00",
    "overlay2-shard01",
    "overlay2-shard02",
    "overlay2-shard03",
    "overlay2-shard04",
    "overlay2-shard05",
    "overlay3-shard00",
    "overlay3-shard01",
    "overlay3-shard02",
    "overlay3-shard03",
    "overlay3-shard04",
    "overlay3-shard05",
    "overlay3-shard06",
    "overlay3-shard07",
    "shard15",
    "shard16",
    "shard00",
    "shard01",
    "shard02",
    "shard03",
    "shard04",
    "shard05",
    "shard06",
    "shard07",
    "shard08",
    "shard09",
    "shard10",
    "shard11",
    "shard12",
    "shard13",
    "shard14",
];
const IDENTITY_WATCHDOG: Duration = Duration::from_secs(60);
const WRITER_RUNTIME_WATCHDOG: Duration = Duration::from_secs(10 * 60);
// The selected WM Bootstrap path emitted 8,214,477 bytes of ordinary runtime
// diagnostics before its report. Keep transport finite with one source-bound
// ceiling while extracting only the single authority-bearing envelope.
const WRITER_RUNTIME_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const WRITER_RUNTIME_REPORT_LIMIT: usize = 1024 * 1024;
const WRITER_RUNTIME_DIAGNOSTIC_TAIL_LIMIT: usize = 4096;
const SI_RUNTIME_SERIES_RUNS: usize = 10;
const SP_RUNTIME_SERIES_RUNS: usize = 10;
const BOOTSTRAP_RUNTIME_SERIES_RUNS: usize = 10;
const CPU_RUNTIME_SERIES_RUNS: usize = 10;
const HOST_ABI_RUNTIME_SERIES_RUNS: usize = 10;
const PI_RUNTIME_SERIES_RUNS: usize = 10;
const RDP_RENDERER_RUNTIME_SERIES_RUNS: usize = 10;
const RSP_RUNTIME_SERIES_RUNS: usize = 10;
pub const WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1: u8 = 1 << 0;
pub const WRITER_AUDIT_SI_COMPLETED_V1: u8 = 1 << 1;
pub const WRITER_AUDIT_SP_COMPLETED_V1: u8 = 1 << 2;
pub const WRITER_AUDIT_CPU_COMPLETED_V1: u8 = 1 << 3;
pub const WRITER_AUDIT_PI_COMPLETED_V1: u8 = 1 << 4;
pub const WRITER_AUDIT_HOST_ABI_COMPLETED_V1: u8 = 1 << 5;
pub const WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1: u8 = 1 << 6;
pub const WRITER_AUDIT_RSP_COMPLETED_V1: u8 = 1 << 7;
const WRITER_AUDIT_COMPLETED_MASK_V1: u8 = WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
    | WRITER_AUDIT_SI_COMPLETED_V1
    | WRITER_AUDIT_SP_COMPLETED_V1
    | WRITER_AUDIT_CPU_COMPLETED_V1
    | WRITER_AUDIT_PI_COMPLETED_V1
    | WRITER_AUDIT_HOST_ABI_COMPLETED_V1
    | WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
    | WRITER_AUDIT_RSP_COMPLETED_V1;
const BUILD_MAX_RSS_MIB: u32 = 4096;
const BUILD_MIN_FREE_PERCENT: u8 = 40;
const MIN_BUILD_TIMEOUT_SECONDS: u64 = 40 * 60;
const MAX_BUILD_TIMEOUT_SECONDS: u64 = 2 * 60 * 60;
const MEMORY_GUARD_SOURCE: &[u8] = include_bytes!("../../../../scripts/memory-guard.zsh");


#[cfg(test)]
mod tests;
