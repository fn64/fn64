//! Parser and report binding for the crash-flushed unsupported-event journal.

use crate::{ClosurePathStatus, GateError, ReleaseGateReport};
use fn64_runtime::{UnsupportedDisposition, UnsupportedSubsystem};
use std::{fmt, str};

const JOURNAL_SCHEMA_V1: &str = "fn64.unsupported-journal.v1";
const JOURNAL_SCHEMA_V2: &str = "fn64.unsupported-journal.v2";
const JOURNAL_SCHEMA_V3: &str = "fn64.unsupported-journal.v3";
const UNSUPPORTED_PATH: &str = "execution.unsupported-event-source";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedUnsupportedJournal {
    pub events: Vec<ParsedUnsupportedJournalEvent>,
    pub completion: UnsupportedJournalCompletion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsupportedJournalCompletion {
    Incomplete,
    V1Unbound {
        guest_cycle: u64,
    },
    V2Bound {
        guest_cycle: u64,
        report_sha256: String,
    },
    V3RunBound {
        guest_cycle: u64,
        report_sha256: String,
        run_event_sha256: String,
    },
}

impl ParsedUnsupportedJournal {
    pub fn release_run_event_sha256(&self) -> Option<&str> {
        match &self.completion {
            UnsupportedJournalCompletion::V3RunBound {
                run_event_sha256, ..
            } => Some(run_event_sha256),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedUnsupportedJournalEvent {
    pub sequence: u64,
    pub guest_cycle: Option<u64>,
    pub subsystem: UnsupportedSubsystem,
    pub disposition: UnsupportedDisposition,
    pub operation: String,
    pub context: String,
}

/// Parse the complete journal grammar without assigning success semantics to
/// an armed-only file. Callers must bind the parsed result to a report before
/// treating its completion as release evidence.
pub fn parse_unsupported_journal(
    bytes: &[u8],
) -> Result<ParsedUnsupportedJournal, UnsupportedJournalError> {
    let text = str::from_utf8(bytes).map_err(UnsupportedJournalError::InvalidUtf8)?;
    if !text.ends_with('\n') {
        return Err(UnsupportedJournalError::TruncatedRecord);
    }
    let lines: Vec<&str> = text[..text.len() - 1].split('\n').collect();
    let header_fields: Vec<&str> = lines
        .first()
        .copied()
        .unwrap_or_default()
        .split('\t')
        .collect();
    let (schema, armed_run_event_sha256) = match header_fields.as_slice() {
        [JOURNAL_SCHEMA_V1, "armed"] => (JOURNAL_SCHEMA_V1, None),
        [JOURNAL_SCHEMA_V2, "armed"] => (JOURNAL_SCHEMA_V2, None),
        [JOURNAL_SCHEMA_V3, "armed", run_event_sha256] => {
            validate_run_event_sha256(run_event_sha256, 1)?;
            (JOURNAL_SCHEMA_V3, Some((*run_event_sha256).to_owned()))
        }
        _ => return Err(UnsupportedJournalError::MissingArmedHeader),
    };

    let mut events = Vec::new();
    let mut completion_cycle = None;
    let mut report_sha256 = None;
    let mut completion_run_event_sha256 = None;
    let mut previous_sequence = None;
    for (offset, line) in lines.iter().enumerate().skip(1) {
        let line_number = offset + 1;
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.first().copied() != Some(schema) {
            return Err(UnsupportedJournalError::MalformedRecord { line: line_number });
        }
        match fields.get(1).copied() {
            Some("armed") => return Err(UnsupportedJournalError::DuplicateArmedHeader),
            Some("complete") => {
                let expected_fields = match schema {
                    JOURNAL_SCHEMA_V1 => 3,
                    JOURNAL_SCHEMA_V2 => 4,
                    JOURNAL_SCHEMA_V3 => 5,
                    _ => unreachable!("journal schema was selected from supported constants"),
                };
                if fields.len() != expected_fields {
                    return Err(UnsupportedJournalError::MalformedRecord { line: line_number });
                }
                if completion_cycle.is_some() {
                    return Err(UnsupportedJournalError::DuplicateCompletion);
                }
                completion_cycle = Some(parse_u64(fields[2], line_number, "completion cycle")?);
                if schema != JOURNAL_SCHEMA_V1 {
                    validate_sha256(fields[3], line_number)?;
                    report_sha256 = Some(fields[3].to_owned());
                }
                if schema == JOURNAL_SCHEMA_V3 {
                    validate_run_event_sha256(fields[4], line_number)?;
                    completion_run_event_sha256 = Some(fields[4].to_owned());
                }
                if offset + 1 != lines.len() {
                    if lines[offset + 1].starts_with(&format!("{schema}\tcomplete\t")) {
                        return Err(UnsupportedJournalError::DuplicateCompletion);
                    }
                    return Err(UnsupportedJournalError::RecordAfterCompletion {
                        line: line_number + 1,
                    });
                }
            }
            Some("event") => {
                if fields.len() != 8 {
                    return Err(UnsupportedJournalError::MalformedRecord { line: line_number });
                }
                let sequence = parse_u64(fields[2], line_number, "event sequence")?;
                if previous_sequence.is_some_and(|previous| sequence <= previous) {
                    return Err(UnsupportedJournalError::NonIncreasingSequence {
                        line: line_number,
                        previous: previous_sequence.expect("matched Some"),
                        observed: sequence,
                    });
                }
                previous_sequence = Some(sequence);
                let guest_cycle = if fields[3] == "unknown" {
                    None
                } else {
                    Some(parse_u64(fields[3], line_number, "event cycle")?)
                };
                let subsystem = match fields[4] {
                    "runtime" => UnsupportedSubsystem::Runtime,
                    "abi" => UnsupportedSubsystem::Abi,
                    "audio" => UnsupportedSubsystem::Audio,
                    "recompiler" => UnsupportedSubsystem::Recompiler,
                    "render" => UnsupportedSubsystem::Render,
                    value => {
                        return Err(UnsupportedJournalError::UnknownSubsystem {
                            line: line_number,
                            value: value.to_owned(),
                        })
                    }
                };
                let disposition = match fields[5] {
                    "loud_trap" => UnsupportedDisposition::LoudTrap,
                    "returned_error" => UnsupportedDisposition::ReturnedError,
                    "needs_lle" => UnsupportedDisposition::NeedsLle,
                    value => {
                        return Err(UnsupportedJournalError::UnknownDisposition {
                            line: line_number,
                            value: value.to_owned(),
                        })
                    }
                };
                events.push(ParsedUnsupportedJournalEvent {
                    sequence,
                    guest_cycle,
                    subsystem,
                    disposition,
                    operation: decode_hex_utf8(fields[6], line_number, "operation")?,
                    context: decode_hex_utf8(fields[7], line_number, "context")?,
                });
            }
            _ => return Err(UnsupportedJournalError::MalformedRecord { line: line_number }),
        }
    }

    let completion = match (schema, completion_cycle) {
        (_, None) => UnsupportedJournalCompletion::Incomplete,
        (JOURNAL_SCHEMA_V1, Some(guest_cycle)) => {
            UnsupportedJournalCompletion::V1Unbound { guest_cycle }
        }
        (JOURNAL_SCHEMA_V2, Some(guest_cycle)) => UnsupportedJournalCompletion::V2Bound {
            guest_cycle,
            report_sha256: report_sha256.expect("v2 completion parser requires report digest"),
        },
        (JOURNAL_SCHEMA_V3, Some(guest_cycle)) => {
            let completion_run_event_sha256 = completion_run_event_sha256
                .expect("v3 completion parser requires run-event digest");
            let armed_run_event_sha256 =
                armed_run_event_sha256.expect("v3 armed header requires run-event digest");
            if completion_run_event_sha256 != armed_run_event_sha256 {
                return Err(UnsupportedJournalError::RunEventDigestMismatch {
                    armed: armed_run_event_sha256,
                    completion: completion_run_event_sha256,
                });
            }
            UnsupportedJournalCompletion::V3RunBound {
                guest_cycle,
                report_sha256: report_sha256.expect("v3 completion parser requires report digest"),
                run_event_sha256: armed_run_event_sha256,
            }
        }
        _ => unreachable!("journal schema was selected from the supported constants"),
    };
    Ok(ParsedUnsupportedJournal { events, completion })
}

/// Require a terminal journal for exactly the fixed cycle and zero-event
/// closure asserted by a valid schema-v17 report.
pub fn verify_release_report_journal(
    report: &ReleaseGateReport,
    journal: &ParsedUnsupportedJournal,
) -> Result<(), UnsupportedJournalError> {
    report
        .require_closed()
        .map_err(UnsupportedJournalError::InvalidReport)?;
    let (completion_cycle, bound_report_sha) = match &journal.completion {
        UnsupportedJournalCompletion::Incomplete => {
            return Err(UnsupportedJournalError::IncompleteObservation)
        }
        UnsupportedJournalCompletion::V1Unbound { .. } => {
            return Err(UnsupportedJournalError::UnboundV1Completion)
        }
        UnsupportedJournalCompletion::V2Bound { .. } => {
            return Err(UnsupportedJournalError::UnboundV2RunIdentity)
        }
        UnsupportedJournalCompletion::V3RunBound {
            guest_cycle,
            report_sha256,
            ..
        } => (*guest_cycle, report_sha256.as_str()),
    };
    if completion_cycle != report.digest.guest_cycle {
        return Err(UnsupportedJournalError::CompletionCycleMismatch {
            report: report.digest.guest_cycle,
            journal: completion_cycle,
        });
    }
    if bound_report_sha != report.report_sha256 {
        return Err(UnsupportedJournalError::ReportDigestMismatch {
            report: report.report_sha256.clone(),
            journal: bound_report_sha.to_owned(),
        });
    }
    if let Some(event) = journal.events.iter().find(|event| {
        event
            .guest_cycle
            .is_some_and(|cycle| cycle > completion_cycle)
    }) {
        return Err(UnsupportedJournalError::FutureEvent {
            sequence: event.sequence,
            event_cycle: event.guest_cycle.expect("matched Some"),
            completion_cycle,
        });
    }
    if !journal.events.is_empty() {
        return Err(UnsupportedJournalError::ReachedUnsupportedEvents {
            count: journal.events.len(),
        });
    }
    let source = report
        .closure
        .iter()
        .find(|path| path.name == UNSUPPORTED_PATH)
        .ok_or(UnsupportedJournalError::MissingUnsupportedPath)?;
    if source.status != ClosurePathStatus::ExercisedZeroUnsupported
        || source.observations == 0
        || !source.unsupported.is_empty()
    {
        return Err(UnsupportedJournalError::ReportDoesNotAssertZero);
    }
    Ok(())
}

fn parse_u64(
    value: &str,
    line: usize,
    field: &'static str,
) -> Result<u64, UnsupportedJournalError> {
    value
        .parse()
        .map_err(|_| UnsupportedJournalError::InvalidInteger {
            line,
            field,
            value: value.to_owned(),
        })
}

fn decode_hex_utf8(
    value: &str,
    line: usize,
    field: &'static str,
) -> Result<String, UnsupportedJournalError> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(UnsupportedJournalError::InvalidHex { line, field });
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16).expect("validated hex") as u8;
        let low = (pair[1] as char).to_digit(16).expect("validated hex") as u8;
        bytes.push((high << 4) | low);
    }
    String::from_utf8(bytes).map_err(|_| UnsupportedJournalError::InvalidText { line, field })
}

fn validate_sha256(value: &str, line: usize) -> Result<(), UnsupportedJournalError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(UnsupportedJournalError::InvalidReportDigest { line });
    }
    Ok(())
}

fn validate_run_event_sha256(value: &str, line: usize) -> Result<(), UnsupportedJournalError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(UnsupportedJournalError::InvalidRunEventDigest { line });
    }
    Ok(())
}

#[derive(Debug)]
pub enum UnsupportedJournalError {
    InvalidUtf8(str::Utf8Error),
    TruncatedRecord,
    MissingArmedHeader,
    DuplicateArmedHeader,
    DuplicateCompletion,
    RecordAfterCompletion {
        line: usize,
    },
    MalformedRecord {
        line: usize,
    },
    InvalidInteger {
        line: usize,
        field: &'static str,
        value: String,
    },
    NonIncreasingSequence {
        line: usize,
        previous: u64,
        observed: u64,
    },
    UnknownSubsystem {
        line: usize,
        value: String,
    },
    UnknownDisposition {
        line: usize,
        value: String,
    },
    InvalidHex {
        line: usize,
        field: &'static str,
    },
    InvalidText {
        line: usize,
        field: &'static str,
    },
    InvalidReportDigest {
        line: usize,
    },
    InvalidRunEventDigest {
        line: usize,
    },
    RunEventDigestMismatch {
        armed: String,
        completion: String,
    },
    InvalidReport(GateError),
    IncompleteObservation,
    CompletionCycleMismatch {
        report: u64,
        journal: u64,
    },
    UnboundV1Completion,
    UnboundV2RunIdentity,
    ReportDigestMismatch {
        report: String,
        journal: String,
    },
    FutureEvent {
        sequence: u64,
        event_cycle: u64,
        completion_cycle: u64,
    },
    ReachedUnsupportedEvents {
        count: usize,
    },
    MissingUnsupportedPath,
    ReportDoesNotAssertZero,
}

impl fmt::Display for UnsupportedJournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8(error) => write!(f, "unsupported journal is not UTF-8: {error}"),
            Self::TruncatedRecord => write!(f, "unsupported journal ends in a truncated record"),
            Self::MissingArmedHeader => write!(f, "unsupported journal lacks its initial armed record"),
            Self::DuplicateArmedHeader => write!(f, "unsupported journal contains a duplicate armed record"),
            Self::DuplicateCompletion => write!(f, "unsupported journal contains duplicate completion records"),
            Self::RecordAfterCompletion { line } => write!(f, "unsupported journal has a record after completion at line {line}"),
            Self::MalformedRecord { line } => write!(f, "unsupported journal has a malformed record at line {line}"),
            Self::InvalidInteger { line, field, value } => write!(f, "unsupported journal has invalid {field} {value:?} at line {line}"),
            Self::NonIncreasingSequence { line, previous, observed } => write!(f, "unsupported journal event sequence at line {line} is not increasing: {observed} after {previous}"),
            Self::UnknownSubsystem { line, value } => write!(f, "unsupported journal has unknown subsystem {value:?} at line {line}"),
            Self::UnknownDisposition { line, value } => write!(f, "unsupported journal has unknown disposition {value:?} at line {line}"),
            Self::InvalidHex { line, field } => write!(f, "unsupported journal has invalid hex for {field} at line {line}"),
            Self::InvalidText { line, field } => write!(f, "unsupported journal decoded {field} is not UTF-8 at line {line}"),
            Self::InvalidReportDigest { line } => write!(f, "unsupported journal has an invalid report SHA-256 at line {line}"),
            Self::InvalidRunEventDigest { line } => write!(f, "unsupported journal has an invalid run-event SHA-256 at line {line}"),
            Self::RunEventDigestMismatch { armed, completion } => write!(f, "unsupported journal completion run-event SHA-256 {completion} differs from armed identity {armed}"),
            Self::InvalidReport(error) => write!(f, "release report paired with journal is invalid: {error}"),
            Self::IncompleteObservation => write!(f, "unsupported journal has no terminal completion record (early abort)"),
            Self::CompletionCycleMismatch { report, journal } => write!(f, "unsupported journal completed at cycle {journal}, but report is fixed at cycle {report}"),
            Self::UnboundV1Completion => write!(f, "v1 unsupported journal completion does not bind a report SHA-256 and is historical evidence only"),
            Self::UnboundV2RunIdentity => write!(f, "v2 unsupported journal completion does not bind a caller-supplied run-event SHA-256 and is historical evidence only"),
            Self::ReportDigestMismatch { report, journal } => write!(f, "unsupported journal binds report {journal}, but paired report is {report}"),
            Self::FutureEvent { sequence, event_cycle, completion_cycle } => write!(f, "unsupported event sequence {sequence} is at future cycle {event_cycle} after completion {completion_cycle}"),
            Self::ReachedUnsupportedEvents { count } => write!(f, "unsupported journal reached {count} unsupported event(s); zero cannot be claimed"),
            Self::MissingUnsupportedPath => write!(f, "release report lacks {UNSUPPORTED_PATH}"),
            Self::ReportDoesNotAssertZero => write!(f, "release report does not contain a counted zero-unsupported observation"),
        }
    }
}

impl std::error::Error for UnsupportedJournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidUtf8(error) => Some(error),
            Self::InvalidReport(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArtifactKind, ClosurePath, FixedCycleDigestGate, ReleaseObservationGeometry,
        LIVE_MINIMUM_CLOSURE_PATHS,
    };

    const RUN_EVENT_SHA256: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";

    fn closed_report(cycle: u64) -> ReleaseGateReport {
        let mut digest = FixedCycleDigestGate::new(cycle);
        for kind in [
            ArtifactKind::Audio,
            ArtifactKind::DeviceState,
            ArtifactKind::TimingTrace,
        ] {
            digest.capture(cycle, kind, &[kind as u8]).unwrap();
        }
        digest
            .capture(cycle, ArtifactKind::Framebuffer, &[0; 2])
            .unwrap();
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
            "journal",
            b"synthetic",
            digest.finish().unwrap(),
            ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
            closure,
        )
        .unwrap()
    }

    #[test]
    fn accepts_terminal_zero_journal_at_report_cycle() {
        let report = closed_report(42);
        let bytes = format!(
            "fn64.unsupported-journal.v3\tarmed\t{RUN_EVENT_SHA256}\nfn64.unsupported-journal.v3\tcomplete\t42\t{}\t{RUN_EVENT_SHA256}\n",
            report.report_sha256,
        );
        let parsed = parse_unsupported_journal(bytes.as_bytes()).unwrap();
        verify_release_report_journal(&report, &parsed).unwrap();
    }

    #[test]
    fn rejects_armed_only_and_event_only_as_early_abort() {
        let armed = parse_unsupported_journal(b"fn64.unsupported-journal.v1\tarmed\n").unwrap();
        assert!(matches!(
            verify_release_report_journal(&closed_report(42), &armed),
            Err(UnsupportedJournalError::IncompleteObservation)
        ));
        let event = parse_unsupported_journal(
            b"fn64.unsupported-journal.v1\tarmed\nfn64.unsupported-journal.v1\tevent\t7\t41\trender\tneeds_lle\t6f70\t637478\n",
        )
        .unwrap();
        assert!(matches!(
            verify_release_report_journal(&closed_report(42), &event),
            Err(UnsupportedJournalError::IncompleteObservation)
        ));
    }

    #[test]
    fn rejects_cycle_mismatch_and_completed_events() {
        let report = closed_report(42);
        let mismatch_bytes = format!(
            "fn64.unsupported-journal.v3\tarmed\t{RUN_EVENT_SHA256}\nfn64.unsupported-journal.v3\tcomplete\t41\t{}\t{RUN_EVENT_SHA256}\n",
            report.report_sha256
        );
        let mismatch = parse_unsupported_journal(mismatch_bytes.as_bytes()).unwrap();
        assert!(matches!(
            verify_release_report_journal(&report, &mismatch),
            Err(UnsupportedJournalError::CompletionCycleMismatch { .. })
        ));
        let reached_bytes = format!(
            "fn64.unsupported-journal.v3\tarmed\t{RUN_EVENT_SHA256}\nfn64.unsupported-journal.v3\tevent\t7\t41\tabi\treturned_error\t6f70\t637478\nfn64.unsupported-journal.v3\tcomplete\t42\t{}\t{RUN_EVENT_SHA256}\n",
            report.report_sha256
        );
        let reached = parse_unsupported_journal(reached_bytes.as_bytes()).unwrap();
        assert!(matches!(
            verify_release_report_journal(&report, &reached),
            Err(UnsupportedJournalError::ReachedUnsupportedEvents { count: 1 })
        ));
    }

    #[test]
    fn rejects_historical_v1_and_stale_same_cycle_binding() {
        let report = closed_report(42);
        let v1 = parse_unsupported_journal(
            b"fn64.unsupported-journal.v1\tarmed\nfn64.unsupported-journal.v1\tcomplete\t42\n",
        )
        .unwrap();
        assert!(matches!(
            verify_release_report_journal(&report, &v1),
            Err(UnsupportedJournalError::UnboundV1Completion)
        ));
        let v2 = parse_unsupported_journal(
            format!(
                "fn64.unsupported-journal.v2\tarmed\nfn64.unsupported-journal.v2\tcomplete\t42\t{}\n",
                report.report_sha256
            )
            .as_bytes(),
        )
        .unwrap();
        assert!(matches!(
            verify_release_report_journal(&report, &v2),
            Err(UnsupportedJournalError::UnboundV2RunIdentity)
        ));
        let stale = parse_unsupported_journal(
            format!(
                "fn64.unsupported-journal.v3\tarmed\t{RUN_EVENT_SHA256}\nfn64.unsupported-journal.v3\tcomplete\t42\t0000000000000000000000000000000000000000000000000000000000000000\t{RUN_EVENT_SHA256}\n"
            )
            .as_bytes(),
        )
        .unwrap();
        assert!(matches!(
            verify_release_report_journal(&report, &stale),
            Err(UnsupportedJournalError::ReportDigestMismatch { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_or_trailing_terminal_records() {
        for bytes in [
            b"fn64.unsupported-journal.v1\tarmed\nfn64.unsupported-journal.v1\tarmed\n".as_slice(),
            b"fn64.unsupported-journal.v1\tarmed\nfn64.unsupported-journal.v1\tcomplete\t42\nfn64.unsupported-journal.v1\tcomplete\t42\n".as_slice(),
        ] {
            assert!(parse_unsupported_journal(bytes).is_err());
        }
    }

    #[test]
    fn rejects_malformed_typed_events_and_future_cycles() {
        for bytes in [
            b"fn64.unsupported-journal.v1\tarmed\nfn64.unsupported-journal.v1\tevent\t7\t41\tunknown\tneeds_lle\t6f70\t637478\n".as_slice(),
            b"fn64.unsupported-journal.v1\tarmed\nfn64.unsupported-journal.v1\tevent\t7\t41\trender\tunknown\t6f70\t637478\n".as_slice(),
            b"fn64.unsupported-journal.v1\tarmed\nfn64.unsupported-journal.v1\tevent\t7\t41\trender\tneeds_lle\tz1\t637478\n".as_slice(),
            b"fn64.unsupported-journal.v1\tarmed\nfn64.unsupported-journal.v1\tevent\t7\t41\trender\tneeds_lle\tff\t637478\n".as_slice(),
        ] {
            assert!(parse_unsupported_journal(bytes).is_err());
        }

        let report = closed_report(42);
        let future_bytes = format!(
            "fn64.unsupported-journal.v3\tarmed\t{RUN_EVENT_SHA256}\nfn64.unsupported-journal.v3\tevent\t7\t43\trender\tneeds_lle\t6f70\t637478\nfn64.unsupported-journal.v3\tcomplete\t42\t{}\t{RUN_EVENT_SHA256}\n",
            report.report_sha256
        );
        let future = parse_unsupported_journal(future_bytes.as_bytes()).unwrap();
        assert!(matches!(
            verify_release_report_journal(&report, &future),
            Err(UnsupportedJournalError::FutureEvent { .. })
        ));
    }

    #[test]
    fn rejects_malformed_or_changed_v3_run_identity() {
        let report = closed_report(42);
        let changed = format!(
            "fn64.unsupported-journal.v3\tarmed\t{RUN_EVENT_SHA256}\nfn64.unsupported-journal.v3\tcomplete\t42\t{}\t2222222222222222222222222222222222222222222222222222222222222222\n",
            report.report_sha256
        );
        assert!(matches!(
            parse_unsupported_journal(changed.as_bytes()),
            Err(UnsupportedJournalError::RunEventDigestMismatch { .. })
        ));
        assert!(matches!(
            parse_unsupported_journal(b"fn64.unsupported-journal.v3\tarmed\tABC\n"),
            Err(UnsupportedJournalError::InvalidRunEventDigest { line: 1 })
        ));
    }
}
