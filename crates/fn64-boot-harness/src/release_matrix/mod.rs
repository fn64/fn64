//! Typed representative-scenario matrix over deterministic release reports.
//!
//! The manifest contains only the immutable project profile and evidence
//! identities, never ROM bytes, captured output, or caller-authored coverage.
//! Dynamic evidence requires schema-v29 report series; coverage is derived
//! from each validated report before it is compared with the fixed profile.


use crate::platform_certification::{
    PlatformCertificationError, VerifiedRt64PlatformCaseAuthority,
};
use crate::{
    verify_release_evidence_series, ArtifactDigest, ArtifactKind, CertificationProfileIdentity,
    CertificationRequirementClass, CertificationRequirementRef, ClosurePath, ClosurePathStatus,
    DeterministicDigest, ExecutionDestinationEvidence, ExecutionDestinationSource,
    FramebufferObservationSource, FullParityV1, ParsedUnsupportedJournal, ReleaseCartridgeSave,
    ReleaseControllerPort, ReleaseEnvironmentEvidence, ReleaseGateReport, ReleaseGraphicsApi,
    ReleaseGraphicsExecutionPolicy, ReleaseHostPlatform, ReleaseMicrocodeFamily,
    ReleaseObservationGeometry, ReleaseRendererEvidence, ReleaseRomClass, ReleaseRomEvidence,
    ReleaseTvRegion, ReleaseWindowsFamily, ReportSeriesError, RspRdpEvidence,
    RspRdpObservationKindEvidence, Rt64PlatformCase, Rt64PlatformTarget,
    VerifiedPrivateReleaseSeries, VerifiedRt64PlatformCaseSeries, LIVE_MINIMUM_CLOSURE_PATHS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

mod types;
mod verify;
mod wire;

pub use types::*;
pub use verify::*;
pub use wire::*;


#[cfg(test)]
mod tests;
