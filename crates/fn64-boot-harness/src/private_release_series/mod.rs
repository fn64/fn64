//! Trusted-process orchestration for one fixed-cycle release series.
//!
//! A retained report series can prove semantic equality and distinct event
//! identities, but it cannot prove who created those identities. During an
//! observed invocation this module owns the missing orchestration boundary:
//! one process creates a random series nonce, launches exactly ten sequential
//! child processes, and verifies each durable report/journal pair before
//! launching the next child. Its retained receipt is an integrity record, not
//! a signature or later proof that this process performed those launches.

pub const PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA: &str = "fn64.private-release-run-contract.v3";
pub const PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA: &str = "fn64.private-release-series-receipt.v1";
pub const PRIVATE_RELEASE_SERIES_COUNT: usize = 10;
pub const RELEASE_MICROCODE_TEXT_PATH_ENV: &str = "FN64_RELEASE_MICROCODE_TEXT_PATH";
pub const RELEASE_MICROCODE_DATA_PATH_ENV: &str = "FN64_RELEASE_MICROCODE_DATA_PATH";
pub const REPOSITORY_SYNTHETIC_RELEASE_SCENARIO: &str =
    "synthetic-runtime-device-render-fixed-cycle-v1";
pub const REPOSITORY_SYNTHETIC_NATIVE_RELEASE_SCENARIO: &str =
    "synthetic-native-archive-runtime-device-render-fixed-cycle-v1";
pub const REPOSITORY_SYNTHETIC_RELEASE_CYCLE: u64 = 1_562_500;
pub const REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES: &[u8] =
    b"repository synthetic runner manifest v1\n";
pub const REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES: &[u8] =
    b"repository synthetic readiness v1\n";
pub const REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES: &[u8] =
    b"fn64 synthetic non-game release input v1";

const RELEASE_REPORT_SCHEMA: &str = crate::release_gate::REPORT_SCHEMA;
const CONTRACT_DIGEST_DOMAIN: &[u8] = b"fn64.private-release-run-contract-digest.v3\0";
const RUN_EVENT_DOMAIN: &[u8] = b"fn64.private-release-run-event.v1\0";
const RECEIPT_DIGEST_DOMAIN: &[u8] = b"fn64.private-release-series-receipt-digest.v1\0";
const RECEIPT_FILE: &str = "receipt.json";

#[cfg(all(test, unix))]
use crate::release_program_build_receipt::ReleaseProgramBuildReceipt;
use crate::release_program_build_receipt::{
    load_release_program_build_receipt, ReleaseProgramBuildLane, ReleaseProgramFileIdentity,
    VerifiedReleaseProgramBuildReceipt, RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA,
};
use crate::{
    parse_unsupported_journal, verify_release_evidence_series, verify_release_report_journal,
    ArtifactKind, ExecutionDestinationSource, ParsedUnsupportedJournal, ReleaseGateReport,
    ReleaseRomClass, ReleaseRomEvidence, ReleaseTvStandard, RspRdpObservationKindEvidence,
    LIVE_MINIMUM_CLOSURE_PATHS, RELEASE_GATE_CYCLE_ENV, RELEASE_REPORT_ENV, RELEASE_ROM_CLASS_ENV,
    RELEASE_RUN_EVENT_SHA256_ENV,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

mod types;
mod series;
mod validate;

pub use types::*;
pub use series::*;
// validate's items are all pub(super); a pub glob would re-export nothing
// public and rustc warns. Siblings still reach them through the parent scope.
use validate::*;


#[cfg(test)]
mod tests;
