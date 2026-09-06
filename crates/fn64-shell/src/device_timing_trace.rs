//! Clean-exit writer for the producer-neutral device timing trace.
//!
//! The shell retains the runtime's already-stamped device events and writes
//! them only when explicitly requested. Configuration is parsed once at boot;
//! teardown never re-reads environment state. `create_new` makes an existing
//! evidence file a loud refusal instead of silently replacing a prior run.

use fn64_timing_trace::{DeviceTraceCompletion, TimingDevice};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const TRACE_PATH_ENV: &str = "FN64_DEVICE_TIMING_TRACE";
const TRACE_ID_ENV: &str = "FN64_DEVICE_TIMING_TRACE_ID";
const TRACE_SCOPE_ENV: &str = "FN64_DEVICE_TRACE_SCOPE";

#[derive(Debug)]
struct Config {
    path: PathBuf,
    trace_id: String,
    observed_devices: Vec<TimingDevice>,
}

#[derive(Debug, Default)]
pub struct DeviceTimingTraceSink {
    config: Option<Config>,
    written: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteReceipt {
    pub events: usize,
    pub bytes: usize,
    pub sha256: String,
}

impl DeviceTimingTraceSink {
    pub fn from_env() -> Result<Self, String> {
        let path = std::env::var_os(TRACE_PATH_ENV);
        let trace_id = std::env::var(TRACE_ID_ENV).ok();
        let scope = std::env::var(TRACE_SCOPE_ENV).ok();
        Self::from_values(path.as_deref(), trace_id.as_deref(), scope.as_deref())
    }

    fn from_values(
        path: Option<&std::ffi::OsStr>,
        trace_id: Option<&str>,
        scope: Option<&str>,
    ) -> Result<Self, String> {
        let Some(path) = path else {
            if trace_id.is_some() {
                return Err(format!("{TRACE_ID_ENV} requires {TRACE_PATH_ENV}"));
            }
            return Ok(Self::default());
        };
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(format!("{TRACE_PATH_ENV} must be an absolute path"));
        }
        let trace_id = trace_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{TRACE_ID_ENV} must be nonempty when {TRACE_PATH_ENV} is set"))?
            .to_owned();
        let observed_devices = parse_scope(scope.unwrap_or("pi,ai,si,sp,vi,mi"))?;

        Ok(Self {
            config: Some(Config {
                path,
                trace_id,
                observed_devices,
            }),
            written: false,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.config.is_some()
    }

    pub fn write_once(
        &mut self,
        fabric_trace: &[fn64_runtime::DeviceTraceEvent],
    ) -> Result<Option<WriteReceipt>, String> {
        if self.written {
            return Ok(None);
        }
        let Some(config) = &self.config else {
            return Ok(None);
        };

        let records = fn64_timing_trace::capture(
            fabric_trace,
            fn64_runtime::Cycles::ZERO,
            &config.observed_devices,
            "fn64-shell device-fabric v3",
            &config.trace_id,
            DeviceTraceCompletion::Completed,
        );
        let events = records.len().saturating_sub(2);
        let jsonl = fn64_timing_trace::to_jsonl(&records)
            .map_err(|error| format!("could not encode trace JSONL: {error}"))?;
        write_new(&config.path, jsonl.as_bytes())?;
        self.written = true;

        Ok(Some(WriteReceipt {
            events,
            bytes: jsonl.len(),
            sha256: format!("{:x}", Sha256::digest(jsonl.as_bytes())),
        }))
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("refused output {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not seal output {}: {error}", path.display()))
}

fn parse_scope(raw: &str) -> Result<Vec<TimingDevice>, String> {
    let mut devices = Vec::new();
    for token in raw.split(',') {
        let token = token.trim();
        let device = match token {
            "pi" => TimingDevice::Pi,
            "ai" => TimingDevice::Ai,
            "si" => TimingDevice::Si,
            "sp" => TimingDevice::Sp,
            "vi" => TimingDevice::Vi,
            "mi" => TimingDevice::Mi,
            "" => return Err(format!("{TRACE_SCOPE_ENV} contains an empty device")),
            _ => {
                return Err(format!(
                    "{TRACE_SCOPE_ENV} has unsupported device {token:?}; expected pi,ai,si,sp,vi,mi"
                ))
            }
        };
        if devices.contains(&device) {
            return Err(format!("{TRACE_SCOPE_ENV} repeats device {token:?}"));
        }
        devices.push(device);
    }
    devices.sort_unstable();
    if devices.is_empty() {
        return Err(format!("{TRACE_SCOPE_ENV} must select at least one device"));
    }
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn disabled_sink_is_inert() {
        let mut sink = DeviceTimingTraceSink::from_values(None, None, None).unwrap();
        assert!(!sink.is_enabled());
        assert_eq!(sink.write_once(&[]).unwrap(), None);
    }

    #[test]
    fn configured_sink_requires_absolute_unique_output_and_explicit_identity() {
        assert!(
            DeviceTimingTraceSink::from_values(Some("relative".as_ref()), Some("run"), None)
                .unwrap_err()
                .contains("absolute")
        );
        assert!(
            DeviceTimingTraceSink::from_values(Some("/tmp/run".as_ref()), None, None)
                .unwrap_err()
                .contains(TRACE_ID_ENV)
        );
        assert!(DeviceTimingTraceSink::from_values(None, Some("run"), None)
            .unwrap_err()
            .contains(TRACE_PATH_ENV));
    }

    #[test]
    fn scope_is_canonical_and_duplicate_free() {
        assert_eq!(
            parse_scope("vi,ai,mi").unwrap(),
            vec![TimingDevice::Ai, TimingDevice::Vi, TimingDevice::Mi]
        );
        assert!(parse_scope("ai,ai").unwrap_err().contains("repeats"));
        assert!(parse_scope("dp").unwrap_err().contains("unsupported"));
    }

    #[test]
    fn output_round_trips_and_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("device.jsonl");
        let fabric_trace = [
            fn64_runtime::DeviceTraceEvent {
                at: fn64_runtime::EmulatedInstant::new(25),
                sequence: 8,
                kind: fn64_runtime::DeviceTraceKind::MiInterruptRaised(
                    fn64_runtime::InterruptSource::Vi,
                ),
            },
            fn64_runtime::DeviceTraceEvent {
                at: fn64_runtime::EmulatedInstant::new(40),
                sequence: 9,
                kind: fn64_runtime::DeviceTraceKind::ViInterrupt,
            },
        ];
        let mut sink = DeviceTimingTraceSink::from_values(
            Some(path.as_os_str()),
            Some("paired-run"),
            Some("vi,ai,mi"),
        )
        .unwrap();
        assert!(sink.is_enabled());
        let receipt = sink.write_once(&fabric_trace).unwrap().unwrap();
        assert_eq!(receipt.events, 2);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(receipt.bytes, bytes.len());
        let ingest = fn64_timing_trace::ingest_jsonl(Cursor::new(bytes)).unwrap();
        assert_eq!(ingest.header.trace_id, "paired-run");
        assert_eq!(
            ingest.header.observed_devices,
            vec![TimingDevice::Ai, TimingDevice::Vi, TimingDevice::Mi]
        );
        assert_eq!(ingest.events.len(), 2);
        assert_eq!(ingest.events[0].cycle, 0);
        assert_eq!(ingest.events[1].cycle, 15);
        assert_eq!(sink.write_once(&fabric_trace).unwrap(), None);

        let mut second = DeviceTimingTraceSink::from_values(
            Some(path.as_os_str()),
            Some("second-run"),
            Some("ai,vi,mi"),
        )
        .unwrap();
        assert!(second
            .write_once(&[])
            .unwrap_err()
            .contains("refused output"));
    }
}
