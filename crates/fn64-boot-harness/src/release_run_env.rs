//! Process environment contract for one runner-owned release invocation.

use std::{ffi::OsString, fmt, path::PathBuf};

use crate::ReleaseRomClass;

pub const RELEASE_GATE_CYCLE_ENV: &str = "FN64_RELEASE_GATE_CYCLE";
pub const RELEASE_REPORT_ENV: &str = "FN64_RELEASE_REPORT";
pub const RELEASE_ROM_CLASS_ENV: &str = "FN64_RELEASE_ROM_CLASS";
pub const RELEASE_RUN_EVENT_SHA256_ENV: &str = "FN64_RELEASE_RUN_EVENT_SHA256";

const OOT_RELEASE_GATE_CYCLE_ENV: &str = "OOT_RELEASE_GATE_CYCLE";
const OOT_RELEASE_REPORT_ENV: &str = "OOT_RELEASE_REPORT";
const OOT_RELEASE_RUN_EVENT_SHA256_ENV: &str = "OOT_RELEASE_RUN_EVENT_SHA256";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseRunEnvironment {
    pub guest_cycle: u64,
    pub report_path: PathBuf,
    pub rom_class: ReleaseRomClass,
    pub run_event_sha256: String,
}

impl ReleaseRunEnvironment {
    pub fn journal_path(&self) -> PathBuf {
        self.report_path.with_extension("unsupported.jsonl")
    }
}

/// Read the generic runner-owned release variables. Ordinary executions with
/// none of the four variables return `None`; partial tuples fail closed.
pub fn release_run_environment_from_process(
) -> Result<Option<ReleaseRunEnvironment>, ReleaseRunEnvironmentError> {
    parse_release_run_environment(read_generic(), read_oot(), read_rom_class(), false)
}

/// Read the generic variables while accepting the historical OoT aliases.
/// A process may use one complete execution namespace, never a mixture or
/// duplicate. ROM class remains runner-owned under its generic name even when
/// the historical execution aliases are accepted.
pub fn release_run_environment_from_process_with_oot_aliases(
) -> Result<Option<ReleaseRunEnvironment>, ReleaseRunEnvironmentError> {
    parse_release_run_environment(read_generic(), read_oot(), read_rom_class(), true)
}

#[derive(Default)]
struct RawReleaseEnvironment {
    cycle: Option<OsString>,
    report: Option<OsString>,
    run_event_sha256: Option<OsString>,
}

impl RawReleaseEnvironment {
    fn any_present(&self) -> bool {
        self.cycle.is_some() || self.report.is_some() || self.run_event_sha256.is_some()
    }

    fn all_present(&self) -> bool {
        self.cycle.is_some() && self.report.is_some() && self.run_event_sha256.is_some()
    }
}

fn read_generic() -> RawReleaseEnvironment {
    RawReleaseEnvironment {
        cycle: std::env::var_os(RELEASE_GATE_CYCLE_ENV),
        report: std::env::var_os(RELEASE_REPORT_ENV),
        run_event_sha256: std::env::var_os(RELEASE_RUN_EVENT_SHA256_ENV),
    }
}

fn read_oot() -> RawReleaseEnvironment {
    RawReleaseEnvironment {
        cycle: std::env::var_os(OOT_RELEASE_GATE_CYCLE_ENV),
        report: std::env::var_os(OOT_RELEASE_REPORT_ENV),
        run_event_sha256: std::env::var_os(OOT_RELEASE_RUN_EVENT_SHA256_ENV),
    }
}

fn read_rom_class() -> Option<OsString> {
    std::env::var_os(RELEASE_ROM_CLASS_ENV)
}

fn parse_release_run_environment(
    generic: RawReleaseEnvironment,
    legacy: RawReleaseEnvironment,
    raw_rom_class: Option<OsString>,
    allow_legacy: bool,
) -> Result<Option<ReleaseRunEnvironment>, ReleaseRunEnvironmentError> {
    if generic.any_present() && legacy.any_present() {
        return Err(ReleaseRunEnvironmentError::MixedNamespaces);
    }
    let (namespace, raw) = if generic.any_present() {
        ("FN64_RELEASE_*", generic)
    } else if legacy.any_present() {
        if !allow_legacy {
            return Err(ReleaseRunEnvironmentError::LegacyNamespaceForbidden);
        }
        ("OOT_RELEASE_*", legacy)
    } else if raw_rom_class.is_none() {
        return Ok(None);
    } else {
        return Err(ReleaseRunEnvironmentError::IncompleteTuple {
            namespace: "FN64_RELEASE_*",
        });
    };
    if !raw.all_present() || raw_rom_class.is_none() {
        return Err(ReleaseRunEnvironmentError::IncompleteTuple { namespace });
    }

    let cycle = unicode(
        raw.cycle.expect("complete release tuple"),
        namespace,
        "cycle",
    )?;
    let guest_cycle = cycle
        .parse::<u64>()
        .map_err(|_| ReleaseRunEnvironmentError::InvalidGuestCycle(cycle.clone()))?;
    let report = unicode(
        raw.report.expect("complete release tuple"),
        namespace,
        "report",
    )?;
    let report_path = PathBuf::from(&report);
    if !report_path.is_absolute() || report_path.as_os_str().is_empty() {
        return Err(ReleaseRunEnvironmentError::InvalidReportPath(report));
    }
    if report_path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ReleaseRunEnvironmentError::InvalidReportPath(report));
    }
    let run_event_sha256 = unicode(
        raw.run_event_sha256.expect("complete release tuple"),
        namespace,
        "run-event SHA-256",
    )?;
    if !canonical_sha256(&run_event_sha256) {
        return Err(ReleaseRunEnvironmentError::InvalidRunEventSha256(
            run_event_sha256,
        ));
    }
    let rom_class = unicode(
        raw_rom_class.expect("complete release tuple"),
        "FN64_RELEASE_*",
        "ROM class",
    )?;
    let rom_class = ReleaseRomClass::from_wire_name(&rom_class)
        .ok_or(ReleaseRunEnvironmentError::InvalidRomClass(rom_class))?;

    Ok(Some(ReleaseRunEnvironment {
        guest_cycle,
        report_path,
        rom_class,
        run_event_sha256,
    }))
}

fn unicode(
    value: OsString,
    namespace: &'static str,
    field: &'static str,
) -> Result<String, ReleaseRunEnvironmentError> {
    value
        .into_string()
        .map_err(|_| ReleaseRunEnvironmentError::NonUnicode { namespace, field })
}

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseRunEnvironmentError {
    MixedNamespaces,
    LegacyNamespaceForbidden,
    IncompleteTuple {
        namespace: &'static str,
    },
    NonUnicode {
        namespace: &'static str,
        field: &'static str,
    },
    InvalidGuestCycle(String),
    InvalidReportPath(String),
    InvalidRomClass(String),
    InvalidRunEventSha256(String),
}

impl fmt::Display for ReleaseRunEnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MixedNamespaces => write!(
                formatter,
                "FN64_RELEASE_* and OOT_RELEASE_* cannot both be present"
            ),
            Self::LegacyNamespaceForbidden => {
                write!(formatter, "OOT_RELEASE_* is not accepted by this host")
            }
            Self::IncompleteTuple { namespace } => write!(
                formatter,
                "{namespace} must provide cycle, report, ROM class, and run-event SHA-256 together"
            ),
            Self::NonUnicode { namespace, field } => {
                write!(formatter, "{namespace} {field} is not valid Unicode")
            }
            Self::InvalidGuestCycle(raw) => {
                write!(
                    formatter,
                    "release guest cycle {raw:?} is not an unsigned integer"
                )
            }
            Self::InvalidReportPath(path) => write!(
                formatter,
                "release report path {path:?} must be absolute and contain no '..' component"
            ),
            Self::InvalidRomClass(value) => write!(
                formatter,
                "release ROM class {value:?} must be unclassified, retail_cartridge, or public_homebrew"
            ),
            Self::InvalidRunEventSha256(value) => write!(
                formatter,
                "release run-event identity {value:?} is not a lowercase SHA-256"
            ),
        }
    }
}

impl std::error::Error for ReleaseRunEnvironmentError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(
        cycle: Option<&str>,
        report: Option<&str>,
        event: Option<&str>,
    ) -> RawReleaseEnvironment {
        RawReleaseEnvironment {
            cycle: cycle.map(OsString::from),
            report: report.map(OsString::from),
            run_event_sha256: event.map(OsString::from),
        }
    }

    #[test]
    fn accepts_exact_generic_or_legacy_tuple_and_derives_journal() {
        let event = "ab".repeat(32);
        let generic = parse_release_run_environment(
            raw(
                Some("42"),
                Some("/private/tmp/report-01.json"),
                Some(&event),
            ),
            RawReleaseEnvironment::default(),
            Some(OsString::from("retail_cartridge")),
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(generic.guest_cycle, 42);
        assert_eq!(generic.rom_class, ReleaseRomClass::RetailCartridge);
        assert_eq!(
            generic.journal_path(),
            PathBuf::from("/private/tmp/report-01.unsupported.jsonl")
        );

        let legacy = parse_release_run_environment(
            RawReleaseEnvironment::default(),
            raw(
                Some("42"),
                Some("/private/tmp/report-01.json"),
                Some(&event),
            ),
            Some(OsString::from("retail_cartridge")),
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(legacy, generic);
    }

    #[test]
    fn rejects_partial_mixed_relative_and_noncanonical_tuples() {
        let event = "ab".repeat(32);
        assert!(matches!(
            parse_release_run_environment(
                raw(Some("42"), None, Some(&event)),
                RawReleaseEnvironment::default(),
                Some(OsString::from("public_homebrew")),
                false,
            ),
            Err(ReleaseRunEnvironmentError::IncompleteTuple { .. })
        ));
        assert!(matches!(
            parse_release_run_environment(
                raw(Some("42"), Some("/tmp/report.json"), Some(&event)),
                raw(Some("42"), Some("/tmp/report.json"), Some(&event)),
                Some(OsString::from("public_homebrew")),
                true,
            ),
            Err(ReleaseRunEnvironmentError::MixedNamespaces)
        ));
        assert!(matches!(
            parse_release_run_environment(
                raw(Some("42"), Some("report.json"), Some(&event)),
                RawReleaseEnvironment::default(),
                Some(OsString::from("public_homebrew")),
                false,
            ),
            Err(ReleaseRunEnvironmentError::InvalidReportPath(_))
        ));
        assert!(matches!(
            parse_release_run_environment(
                raw(Some("42"), Some("/tmp/report.json"), Some(&"AB".repeat(32))),
                RawReleaseEnvironment::default(),
                Some(OsString::from("public_homebrew")),
                false,
            ),
            Err(ReleaseRunEnvironmentError::InvalidRunEventSha256(_))
        ));
    }

    #[test]
    fn requires_runner_owned_class_for_generic_and_legacy_tuples() {
        let event = "ab".repeat(32);
        for (generic, legacy, allow_legacy) in [
            (
                raw(Some("42"), Some("/tmp/report.json"), Some(&event)),
                RawReleaseEnvironment::default(),
                false,
            ),
            (
                RawReleaseEnvironment::default(),
                raw(Some("42"), Some("/tmp/report.json"), Some(&event)),
                true,
            ),
        ] {
            assert!(matches!(
                parse_release_run_environment(generic, legacy, None, allow_legacy),
                Err(ReleaseRunEnvironmentError::IncompleteTuple { .. })
            ));
        }

        assert!(matches!(
            parse_release_run_environment(
                RawReleaseEnvironment::default(),
                RawReleaseEnvironment::default(),
                Some(OsString::from("retail_cartridge")),
                false,
            ),
            Err(ReleaseRunEnvironmentError::IncompleteTuple { .. })
        ));
    }

    #[test]
    fn rejects_unknown_class_instead_of_relabelling_it() {
        let event = "ab".repeat(32);
        assert!(matches!(
            parse_release_run_environment(
                raw(Some("42"), Some("/tmp/report.json"), Some(&event)),
                RawReleaseEnvironment::default(),
                Some(OsString::from("retail")),
                false,
            ),
            Err(ReleaseRunEnvironmentError::InvalidRomClass(value)) if value == "retail"
        ));
    }
}
