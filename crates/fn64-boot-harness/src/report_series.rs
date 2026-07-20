//! Verification for a consecutive fixed-cycle release-report series.

use crate::{
    verify_release_report_journal, GateError, ParsedUnsupportedJournal, ReleaseGateReport,
    UnsupportedJournalError, LIVE_MINIMUM_CLOSURE_PATHS,
};
use std::{collections::BTreeSet, fmt};

/// Evidence retained after every report in a series has passed its own
/// integrity/closure gate and agreed on the complete semantic report digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedReportSeries {
    pub count: usize,
    pub report_sha256: String,
    pub scenario: String,
    pub guest_cycle: u64,
    /// Caller-supplied event identities retained in evidence order.
    pub run_event_sha256s: Vec<String>,
}

/// Verify a retained report series only when every report has its own parsed,
/// terminal unsupported journal. This closes the crash window between a
/// durable report write and the journal completion flush.
pub fn verify_release_evidence_series(
    evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
    minimum: usize,
) -> Result<VerifiedReportSeries, ReportSeriesError> {
    verify_release_report_series(evidence, minimum)
}

/// Verify a retained release series with one crash-flushed, run-bound journal
/// per report. Report-only collections are deliberately not a certifying API:
/// identical semantic digests do not establish distinct executions.
pub fn verify_release_report_series(
    evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
    minimum: usize,
) -> Result<VerifiedReportSeries, ReportSeriesError> {
    let reports: Vec<ReleaseGateReport> =
        evidence.iter().map(|(report, _)| report.clone()).collect();
    let mut verified = verify_semantic_report_series(&reports, minimum)?;
    let mut run_event_sha256s = Vec::with_capacity(evidence.len());
    let mut unique_run_events = BTreeSet::new();
    for (index, (report, journal)) in evidence.iter().enumerate() {
        verify_release_report_journal(report, journal)
            .map_err(|source| ReportSeriesError::InvalidJournal { index, source })?;
        let run_event_sha256 = journal
            .release_run_event_sha256()
            .expect("release journal verifier accepts only v3 run-bound completion")
            .to_owned();
        if !unique_run_events.insert(run_event_sha256.clone()) {
            return Err(ReportSeriesError::DuplicateRunEventIdentity {
                index,
                run_event_sha256,
            });
        }
        run_event_sha256s.push(run_event_sha256);
    }
    verified.run_event_sha256s = run_event_sha256s;
    Ok(verified)
}

fn verify_semantic_report_series(
    reports: &[ReleaseGateReport],
    minimum: usize,
) -> Result<VerifiedReportSeries, ReportSeriesError> {
    if minimum == 0 {
        return Err(ReportSeriesError::ZeroMinimum);
    }
    if reports.len() < minimum {
        return Err(ReportSeriesError::TooFewReports {
            minimum,
            actual: reports.len(),
        });
    }

    for (index, report) in reports.iter().enumerate() {
        report
            .require_closed()
            .map_err(|source| ReportSeriesError::InvalidReport { index, source })?;
        let declared: BTreeSet<&str> = report
            .closure
            .iter()
            .map(|path| path.name.as_str())
            .collect();
        let missing: Vec<String> = LIVE_MINIMUM_CLOSURE_PATHS
            .iter()
            .filter(|path| !declared.contains(**path))
            .map(|path| (*path).to_owned())
            .collect();
        if !missing.is_empty() {
            return Err(ReportSeriesError::MissingRequiredPaths {
                index,
                paths: missing,
            });
        }
    }

    let first = &reports[0];
    for (index, report) in reports.iter().enumerate().skip(1) {
        if report.report_sha256 != first.report_sha256 {
            return Err(ReportSeriesError::DigestMismatch {
                index,
                expected: first.report_sha256.clone(),
                observed: report.report_sha256.clone(),
            });
        }
    }

    Ok(VerifiedReportSeries {
        count: reports.len(),
        report_sha256: first.report_sha256.clone(),
        scenario: first.scenario.clone(),
        guest_cycle: first.digest.guest_cycle,
        run_event_sha256s: Vec::new(),
    })
}

#[derive(Debug)]
pub enum ReportSeriesError {
    ZeroMinimum,
    TooFewReports {
        minimum: usize,
        actual: usize,
    },
    InvalidReport {
        index: usize,
        source: GateError,
    },
    MissingRequiredPaths {
        index: usize,
        paths: Vec<String>,
    },
    DigestMismatch {
        index: usize,
        expected: String,
        observed: String,
    },
    InvalidJournal {
        index: usize,
        source: UnsupportedJournalError,
    },
    DuplicateRunEventIdentity {
        index: usize,
        run_event_sha256: String,
    },
}

impl fmt::Display for ReportSeriesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroMinimum => write!(f, "release-report series minimum must be nonzero"),
            Self::TooFewReports { minimum, actual } => write!(
                f,
                "release-report series has {actual} reports; at least {minimum} are required"
            ),
            Self::InvalidReport { index, source } => {
                write!(f, "release report at series index {index} is invalid: {source}")
            }
            Self::MissingRequiredPaths { index, paths } => write!(
                f,
                "release report at series index {index} omits required live paths {paths:?}"
            ),
            Self::DigestMismatch {
                index,
                expected,
                observed,
            } => write!(
                f,
                "release report at series index {index} differs: expected {expected}, observed {observed}"
            ),
            Self::InvalidJournal { index, source } => write!(
                f,
                "unsupported journal at series index {index} is invalid: {source}"
            ),
            Self::DuplicateRunEventIdentity {
                index,
                run_event_sha256,
            } => write!(
                f,
                "release evidence at series index {index} repeats run-event identity {run_event_sha256}"
            ),
        }
    }
}

impl std::error::Error for ReportSeriesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidReport { source, .. } => Some(source),
            Self::InvalidJournal { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArtifactKind, ClosurePath, ClosurePathStatus, FixedCycleDigestGate,
        ReleaseObservationGeometry,
    };

    fn closed_report(cycle: u64, framebuffer: &[u8]) -> ReleaseGateReport {
        let mut digest = FixedCycleDigestGate::new(cycle);
        digest
            .capture(cycle, ArtifactKind::Framebuffer, framebuffer)
            .unwrap();
        for kind in [
            ArtifactKind::Audio,
            ArtifactKind::DeviceState,
            ArtifactKind::TimingTrace,
        ] {
            digest.capture(cycle, kind, &[kind as u8]).unwrap();
        }
        digest
            .capture(
                cycle,
                ArtifactKind::Memory,
                &vec![0; crate::DEFAULT_RDRAM_SIZE],
            )
            .unwrap();
        let closure = LIVE_MINIMUM_CLOSURE_PATHS
            .iter()
            .map(|name| ClosurePath {
                name: (*name).to_owned(),
                observations: 1,
                status: ClosurePathStatus::ExercisedZeroUnsupported,
                unsupported: Vec::new(),
            })
            .collect();
        ReleaseGateReport::new(
            "series",
            b"private input",
            digest.finish().unwrap(),
            ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
            closure,
        )
        .unwrap()
    }

    fn evidence_for(
        reports: Vec<ReleaseGateReport>,
    ) -> Vec<(ReleaseGateReport, ParsedUnsupportedJournal)> {
        reports
            .into_iter()
            .enumerate()
            .map(|(index, report)| {
                let journal = ParsedUnsupportedJournal {
                    events: Vec::new(),
                    completion: crate::UnsupportedJournalCompletion::V3RunBound {
                        guest_cycle: report.digest.guest_cycle,
                        report_sha256: report.report_sha256.clone(),
                        run_event_sha256: format!("{:064x}", index + 1),
                    },
                };
                (report, journal)
            })
            .collect()
    }

    #[test]
    fn accepts_ten_unique_runs_with_one_semantic_report_digest() {
        let report = closed_report(955_332_880, &[1, 2]);
        let evidence = evidence_for(vec![report.clone(); 10]);
        let verified = verify_release_report_series(&evidence, 10).unwrap();
        assert_eq!(verified.count, 10);
        assert_eq!(verified.report_sha256, report.report_sha256);
        assert_eq!(verified.guest_cycle, 955_332_880);
    }

    #[test]
    fn rejects_too_few_reports() {
        let reports = evidence_for(vec![closed_report(1, &[1, 2]); 9]);
        assert!(matches!(
            verify_release_report_series(&reports, 10),
            Err(ReportSeriesError::TooFewReports {
                minimum: 10,
                actual: 9
            })
        ));
    }

    #[test]
    fn rejects_one_semantically_different_report() {
        let first = closed_report(1, &[1, 2]);
        let mut reports = vec![first; 10];
        reports[7] = closed_report(1, &[3, 4]);
        let reports = evidence_for(reports);
        assert!(matches!(
            verify_release_report_series(&reports, 10),
            Err(ReportSeriesError::DigestMismatch { index: 7, .. })
        ));
    }

    #[test]
    fn rejects_a_closed_report_with_stale_integrity() {
        let mut report = closed_report(1, &[1, 2]);
        report.scenario.push_str("-mutated");
        let reports = evidence_for(vec![report; 10]);
        assert!(matches!(
            verify_release_report_series(&reports, 10),
            Err(ReportSeriesError::InvalidReport { index: 0, .. })
        ));
    }

    #[test]
    fn rejects_closed_but_incomplete_live_path_denominator() {
        let base = closed_report(1, &[1, 2]);
        let closure = LIVE_MINIMUM_CLOSURE_PATHS[..LIVE_MINIMUM_CLOSURE_PATHS.len() - 1]
            .iter()
            .map(|name| ClosurePath {
                name: (*name).to_owned(),
                observations: 1,
                status: ClosurePathStatus::ExercisedZeroUnsupported,
                unsupported: Vec::new(),
            })
            .collect();
        let report = ReleaseGateReport::new(
            "series",
            b"private input",
            base.digest,
            base.observations,
            closure,
        )
        .unwrap();
        let reports = evidence_for(vec![report; 10]);
        assert!(matches!(
            verify_release_report_series(&reports, 10),
            Err(ReportSeriesError::MissingRequiredPaths { index: 0, .. })
        ));
    }

    #[test]
    fn evidence_series_rejects_one_armed_only_journal() {
        let report = closed_report(42, &[1, 2]);
        let mut evidence = evidence_for(vec![report; 10]);
        evidence[6].1.completion = crate::UnsupportedJournalCompletion::Incomplete;
        assert!(matches!(
            verify_release_evidence_series(&evidence, 10),
            Err(ReportSeriesError::InvalidJournal { index: 6, .. })
        ));
    }

    #[test]
    fn rejects_replayed_report_and_journal_pairs() {
        let report = closed_report(42, &[1, 2]);
        let pair = evidence_for(vec![report]).pop().unwrap();
        let evidence = vec![pair; 10];
        assert!(matches!(
            verify_release_report_series(&evidence, 10),
            Err(ReportSeriesError::DuplicateRunEventIdentity { index: 1, .. })
        ));
    }
}
