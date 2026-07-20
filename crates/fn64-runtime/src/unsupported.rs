//! Typed evidence for reached unsupported behavior.
//!
//! This is a diagnostic side channel: recording never changes the existing
//! loud trap, returned error, or LLE handoff.  An optional journal is flushed
//! at arm, at every event, and at successful gate completion so a release run
//! can distinguish a proved zero from an early process abort.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use crate::{trace::next_sequence, Cycles};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedSubsystem {
    Runtime,
    Abi,
    Audio,
    Recompiler,
    Render,
}

impl UnsupportedSubsystem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Abi => "abi",
            Self::Audio => "audio",
            Self::Recompiler => "recompiler",
            Self::Render => "render",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedDisposition {
    LoudTrap,
    ReturnedError,
    NeedsLle,
}

impl UnsupportedDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LoudTrap => "loud_trap",
            Self::ReturnedError => "returned_error",
            Self::NeedsLle => "needs_lle",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedEvent {
    pub sequence: u64,
    pub subsystem: UnsupportedSubsystem,
    pub operation: String,
    pub context: String,
    /// `None` is explicit evidence that the source has no guest clock. ABI
    /// boundaries should always supply the live `DeviceFabric` cycle.
    pub guest_cycle: Option<Cycles>,
    pub disposition: UnsupportedDisposition,
}

struct UnsupportedSource {
    armed: bool,
    events: Vec<UnsupportedEvent>,
    journal: Option<File>,
    journal_schema: &'static str,
    run_event_sha256: Option<String>,
    journal_error: Option<String>,
}

impl Default for UnsupportedSource {
    fn default() -> Self {
        Self {
            armed: false,
            events: Vec::new(),
            journal: None,
            journal_schema: "fn64.unsupported-journal.v2",
            run_event_sha256: None,
            journal_error: None,
        }
    }
}

thread_local! {
    static SOURCE: RefCell<UnsupportedSource> = RefCell::new(UnsupportedSource::default());
}

/// Start a fresh observation window. The journal header is durable before
/// this returns; if the process later aborts without a completion record, the
/// retained file proves only an early/unobserved run, never zero unsupported.
pub fn arm_unsupported_events(journal_path: Option<&Path>) -> io::Result<()> {
    arm_unsupported_events_inner(journal_path, None)
}

/// Start a release-admissible observation window bound to one caller-supplied
/// execution-event identity. The identity is provenance, not proof that a new
/// process or physical run occurred; paired-series verification additionally
/// requires it to be unique across the retained cohort.
pub fn arm_unsupported_events_with_run_identity(
    journal_path: Option<&Path>,
    run_event_sha256: &str,
) -> io::Result<()> {
    if !canonical_sha256(run_event_sha256) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported journal run-event identity must be a lowercase SHA-256",
        ));
    }
    arm_unsupported_events_inner(journal_path, Some(run_event_sha256))
}

fn arm_unsupported_events_inner(
    journal_path: Option<&Path>,
    run_event_sha256: Option<&str>,
) -> io::Result<()> {
    SOURCE.with(|source| *source.borrow_mut() = UnsupportedSource::default());
    let schema = if run_event_sha256.is_some() {
        "fn64.unsupported-journal.v3"
    } else {
        "fn64.unsupported-journal.v2"
    };
    let mut journal = match journal_path {
        Some(path) => {
            let mut file = File::create(path)?;
            if let Some(run_event_sha256) = run_event_sha256 {
                writeln!(file, "{schema}\tarmed\t{run_event_sha256}")?;
            } else {
                writeln!(file, "{schema}\tarmed")?;
            }
            file.flush()?;
            Some(file)
        }
        None => None,
    };
    SOURCE.with(|source| {
        let mut source = source.borrow_mut();
        source.armed = true;
        source.events.clear();
        source.journal_schema = schema;
        source.run_event_sha256 = run_event_sha256.map(str::to_owned);
        source.journal_error = None;
        source.journal = journal.take();
    });
    Ok(())
}

pub fn unsupported_events_armed() -> bool {
    SOURCE.with(|source| source.borrow().armed)
}

pub fn copy_unsupported_events() -> Vec<UnsupportedEvent> {
    SOURCE.with(|source| source.borrow().events.clone())
}

pub fn unsupported_journal_error() -> Option<String> {
    SOURCE.with(|source| source.borrow().journal_error.clone())
}

pub fn record_unsupported_event(
    subsystem: UnsupportedSubsystem,
    operation: impl Into<String>,
    context: impl Into<String>,
    guest_cycle: Option<Cycles>,
    disposition: UnsupportedDisposition,
) {
    SOURCE.with(|source| {
        let mut source = source.borrow_mut();
        if !source.armed {
            return;
        }
        let event = UnsupportedEvent {
            sequence: next_sequence(),
            subsystem,
            operation: operation.into(),
            context: context.into(),
            guest_cycle,
            disposition,
        };
        let line = encode_event(source.journal_schema, &event);
        if let Some(journal) = source.journal.as_mut() {
            if let Err(error) = journal
                .write_all(line.as_bytes())
                .and_then(|_| journal.flush())
            {
                // The diagnostic sink must never replace the original trap or
                // handoff. The release gate rejects this stored I/O failure.
                if source.journal_error.is_none() {
                    source.journal_error = Some(error.to_string());
                }
            }
        }
        source.events.push(event);
    });
}

/// Append the only record that proves the observation window reached its
/// fixed-cycle gate, bound to the report that was already flushed. An armed
/// header without this record is an early abort.
pub fn complete_unsupported_observation(guest_cycle: Cycles, report_sha256: &str) {
    assert!(
        report_sha256.len() == 64
            && report_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "unsupported journal completion requires a lowercase report SHA-256"
    );
    SOURCE.with(|source| {
        let mut source = source.borrow_mut();
        if !source.armed {
            return;
        }
        let line = if let Some(run_event_sha256) = &source.run_event_sha256 {
            format!(
                "{}\tcomplete\t{}\t{}\t{}\n",
                source.journal_schema,
                guest_cycle.get(),
                report_sha256,
                run_event_sha256
            )
        } else {
            format!(
                "{}\tcomplete\t{}\t{}\n",
                source.journal_schema,
                guest_cycle.get(),
                report_sha256
            )
        };
        if let Some(journal) = source.journal.as_mut() {
            if let Err(error) = journal
                .write_all(line.as_bytes())
                .and_then(|_| journal.flush())
            {
                if source.journal_error.is_none() {
                    source.journal_error = Some(error.to_string());
                }
            }
        }
        source.armed = false;
        source.journal = None;
    });
}

fn encode_event(schema: &str, event: &UnsupportedEvent) -> String {
    format!(
        "{}\tevent\t{}\t{}\t{}\t{}\t{}\t{}\n",
        schema,
        event.sequence,
        event
            .guest_cycle
            .map_or_else(|| "unknown".to_owned(), |cycle| cycle.get().to_string()),
        event.subsystem.as_str(),
        event.disposition.as_str(),
        hex(event.operation.as_bytes()),
        hex(event.context.as_bytes()),
    )
}

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_source_does_not_capture() {
        SOURCE.with(|source| *source.borrow_mut() = UnsupportedSource::default());
        record_unsupported_event(
            UnsupportedSubsystem::Runtime,
            "runtime.test.disabled",
            "not armed",
            Some(Cycles::new(1)),
            UnsupportedDisposition::LoudTrap,
        );
        assert!(copy_unsupported_events().is_empty());
    }

    #[test]
    fn journal_distinguishes_armed_event_and_completion() {
        let path = std::env::temp_dir().join(format!(
            "fn64-unsupported-{}-{}.journal",
            std::process::id(),
            next_sequence()
        ));
        arm_unsupported_events(Some(&path)).unwrap();
        let armed = std::fs::read_to_string(&path).unwrap();
        assert!(armed.contains("\tarmed\n"));
        assert!(!armed.contains("\tcomplete\t"));

        record_unsupported_event(
            UnsupportedSubsystem::Render,
            "render.test.needs-lle",
            "ucode=0011",
            Some(Cycles::new(42)),
            UnsupportedDisposition::NeedsLle,
        );
        let reached = std::fs::read_to_string(&path).unwrap();
        assert!(reached.contains("\tevent\t"));
        assert!(!reached.contains("\tcomplete\t"));
        assert_eq!(copy_unsupported_events().len(), 1);

        complete_unsupported_observation(
            Cycles::new(42),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        );
        let complete = std::fs::read_to_string(&path).unwrap();
        assert!(complete.contains("\tcomplete\t42\t0123456789abcdef"));
        assert!(unsupported_journal_error().is_none());
        record_unsupported_event(
            UnsupportedSubsystem::Runtime,
            "runtime.test.after-completion",
            "must not extend a terminal observation",
            Some(Cycles::new(43)),
            UnsupportedDisposition::LoudTrap,
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), complete);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn release_journal_binds_caller_run_identity_at_arm_and_completion() {
        let path = std::env::temp_dir().join(format!(
            "fn64-unsupported-v3-{}-{}.journal",
            std::process::id(),
            next_sequence()
        ));
        let run_event_sha256 = "ab".repeat(32);
        arm_unsupported_events_with_run_identity(Some(&path), &run_event_sha256).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("fn64.unsupported-journal.v3\tarmed\t{run_event_sha256}\n")
        );
        complete_unsupported_observation(Cycles::new(42), &"cd".repeat(32));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!(
                "fn64.unsupported-journal.v3\tarmed\t{run_event_sha256}\n\
                 fn64.unsupported-journal.v3\tcomplete\t42\t{}\t{run_event_sha256}\n",
                "cd".repeat(32)
            )
        );
        let _ = std::fs::remove_file(path);

        let invalid = arm_unsupported_events_with_run_identity(None, "AB").unwrap_err();
        assert_eq!(invalid.kind(), io::ErrorKind::InvalidInput);
        assert!(invalid.to_string().contains("run-event identity"));
    }

    #[test]
    fn runtime_loud_site_records_before_preserving_its_panic() {
        arm_unsupported_events(None).unwrap();
        let trapped = std::panic::catch_unwind(|| {
            let mut pak = crate::TransferPak::new();
            let mut block = [0; crate::TRANSFER_PAK_BLOCK_SIZE];
            pak.read_block(0x0000, &mut block);
        });
        assert!(trapped.is_err());
        let events = copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].operation, "runtime.transfer-pak.read-address");
        assert_eq!(events[0].disposition, UnsupportedDisposition::LoudTrap);
    }
}
