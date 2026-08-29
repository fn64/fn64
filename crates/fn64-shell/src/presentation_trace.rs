//! Opt-in correlation trace for host audio and video presentation.
//!
//! This stream is deliberately separate from `fn64-timing-trace`: device
//! events are deterministic guest evidence, while callback and window
//! timestamps are host observations. The trace is bounded in memory and
//! sealed with `create_new` during clean exit.

use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const TRACE_PATH_ENV: &str = "FN64_PRESENTATION_TRACE";
const TRACE_ID_ENV: &str = "FN64_PRESENTATION_TRACE_ID";
const MAX_RECORDS: usize = 100_000;

#[derive(Debug)]
struct Config {
    path: PathBuf,
}

#[derive(Debug, Default)]
pub struct PresentationTraceSink {
    config: Option<Config>,
    epoch: Option<std::time::Instant>,
    records: Vec<String>,
    last_audio_generation: Option<u64>,
    last_audio_dma: Option<fn64_runtime::AiDmaId>,
    sealed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealReceipt {
    pub records: usize,
    pub bytes: usize,
    pub sha256: String,
}

impl PresentationTraceSink {
    pub fn from_env() -> Result<Self, String> {
        let path = std::env::var_os(TRACE_PATH_ENV);
        let trace_id = std::env::var(TRACE_ID_ENV).ok();
        Self::from_values(
            path.as_deref(),
            trace_id.as_deref(),
            std::time::Instant::now(),
        )
    }

    fn from_values(
        path: Option<&std::ffi::OsStr>,
        trace_id: Option<&str>,
        epoch: std::time::Instant,
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
            .ok_or_else(|| {
                format!("{TRACE_ID_ENV} must be nonempty when {TRACE_PATH_ENV} is set")
            })?;
        if !trace_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-:".contains(&byte))
        {
            return Err(format!(
                "{TRACE_ID_ENV} may contain only ASCII letters, digits, '.', '_', '-', and ':'"
            ));
        }
        Ok(Self {
            config: Some(Config { path }),
            epoch: Some(epoch),
            records: vec![format!(
                "{{\"record\":\"header\",\"schema\":\"fn64.host-presentation.v2\",\"trace_id\":\"{trace_id}\",\"host_time\":\"nanoseconds_from_trace_epoch\",\"emulated_time\":\"r4300_master_cycle\",\"emulated_hz\":{}}}",
                fn64_runtime::CPU_CLOCK_HZ,
            )],
            last_audio_generation: None,
            last_audio_dma: None,
            sealed: false,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.config.is_some()
    }

    pub fn observe_audio(
        &mut self,
        state: Option<fn64_audio::AudioPresentationState>,
        observed_at: std::time::Instant,
    ) {
        if !self.is_enabled() {
            return;
        }
        let Some(state) = state else {
            if self.last_audio_generation.take().is_some() {
                self.last_audio_dma = None;
                let observed_ns = self.relative_ns(observed_at);
                self.push(format!(
                    "{{\"record\":\"audio_unavailable\",\"observed_host_ns\":{observed_ns}}}"
                ));
            }
            return;
        };
        if self.last_audio_generation != Some(state.continuity_generation) {
            self.last_audio_generation = Some(state.continuity_generation);
            self.last_audio_dma = None;
            let observed_ns = self.relative_ns(observed_at);
            self.push(format!(
                "{{\"record\":\"audio_generation\",\"generation\":{},\"anchor_valid\":{},\"observed_host_ns\":{observed_ns}}}",
                state.continuity_generation,
                state.anchor.is_some(),
            ));
        }
        let Some(anchor) = state.anchor else {
            return;
        };
        assert_eq!(
            anchor.continuity_generation, state.continuity_generation,
            "audio presentation state returned an anchor from another generation"
        );
        if self.last_audio_dma == Some(anchor.dma_id) {
            return;
        }
        self.last_audio_dma = Some(anchor.dma_id);
        let observed_ns = self.relative_ns(observed_at);
        let playback_ns = self.relative_ns(anchor.predicted_playback_at);
        self.push(format!(
            "{{\"record\":\"audio_anchor\",\"generation\":{},\"dma_id\":{},\"emulated_cycle\":{},\"predicted_playback_host_ns\":{playback_ns},\"observed_host_ns\":{observed_ns}}}",
            anchor.continuity_generation,
            anchor.dma_id.get(),
            anchor.emulated_at.get(),
        ));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_vi_present(
        &mut self,
        stage: fn64_abi::PresentedViFieldStage,
        presentation_generation: u64,
        retrace_at: fn64_runtime::EmulatedInstant,
        swap_count: u64,
        rgba_hash: u64,
        width: usize,
        height: usize,
        present_return_at: std::time::Instant,
    ) {
        if !self.is_enabled() {
            return;
        }
        let present_return_ns = self.relative_ns(present_return_at);
        self.push(format!(
            "{{\"record\":\"vi_present\",\"stage\":\"{}\",\"presentation_generation\":{presentation_generation},\"retrace_cycle\":{},\"swap_count\":{swap_count},\"rgba_hash\":\"{rgba_hash:016x}\",\"width\":{width},\"height\":{height},\"present_return_host_ns\":{present_return_ns}}}",
            stage.serialized_name(),
            retrace_at.get(),
        ));
    }

    pub fn seal_once(&mut self) -> Result<Option<SealReceipt>, String> {
        if self.sealed || !self.is_enabled() {
            return Ok(None);
        }
        let config = self.config.as_ref().expect("enabled trace retains config");
        self.records.push(format!(
            "{{\"record\":\"end\",\"data_records\":{}}}",
            self.records.len().saturating_sub(1)
        ));
        let mut bytes = self.records.join("\n").into_bytes();
        bytes.push(b'\n');
        write_new(&config.path, &bytes)?;
        self.sealed = true;
        Ok(Some(SealReceipt {
            records: self.records.len(),
            bytes: bytes.len(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        }))
    }

    fn relative_ns(&self, instant: std::time::Instant) -> i128 {
        let epoch = self.epoch.expect("enabled trace retains a host epoch");
        if instant >= epoch {
            i128::try_from(instant.duration_since(epoch).as_nanos()).unwrap_or(i128::MAX)
        } else {
            -i128::try_from(epoch.duration_since(instant).as_nanos()).unwrap_or(i128::MAX)
        }
    }

    fn push(&mut self, record: String) {
        assert!(
            self.records.len() < MAX_RECORDS,
            "host presentation trace exceeded its {MAX_RECORDS}-record bound"
        );
        self.records.push(record);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_trace_is_inert_and_configuration_is_explicit() {
        let epoch = std::time::Instant::now();
        let mut disabled = PresentationTraceSink::from_values(None, None, epoch).unwrap();
        assert!(!disabled.is_enabled());
        assert_eq!(disabled.seal_once().unwrap(), None);
        assert!(
            PresentationTraceSink::from_values(Some("relative".as_ref()), Some("run"), epoch)
                .unwrap_err()
                .contains("absolute")
        );
        assert!(PresentationTraceSink::from_values(None, Some("run"), epoch)
            .unwrap_err()
            .contains(TRACE_PATH_ENV));
    }

    #[test]
    fn trace_joins_generation_anchor_and_exact_vi_present() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("presentation.jsonl");
        let epoch = std::time::Instant::now();
        let mut sink =
            PresentationTraceSink::from_values(Some(path.as_os_str()), Some("joined-run"), epoch)
                .unwrap();
        sink.observe_audio(
            Some(fn64_audio::AudioPresentationState {
                continuity_generation: 2,
                anchor: None,
            }),
            epoch + std::time::Duration::from_nanos(10),
        );
        sink.observe_audio(
            Some(fn64_audio::AudioPresentationState {
                continuity_generation: 2,
                anchor: Some(fn64_audio::AudioPresentationAnchor {
                    dma_id: fn64_runtime::AiDmaId::new(7),
                    emulated_at: fn64_runtime::EmulatedInstant::new(100),
                    predicted_playback_at: epoch + std::time::Duration::from_nanos(20),
                    continuity_generation: 2,
                }),
            }),
            epoch + std::time::Duration::from_nanos(15),
        );
        sink.record_vi_present(
            fn64_abi::PresentedViFieldStage::PostVi,
            9,
            fn64_runtime::EmulatedInstant::new(200),
            11,
            0x1234,
            320,
            240,
            epoch + std::time::Duration::from_nanos(30),
        );
        let receipt = sink.seal_once().unwrap().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(receipt.records, 5);
        assert!(text.contains("\"record\":\"audio_generation\""));
        assert!(text.contains("\"schema\":\"fn64.host-presentation.v2\""));
        assert!(text.contains(
            "\"record\":\"vi_present\",\"stage\":\"post_vi\",\"presentation_generation\":9"
        ));
        assert!(text.contains("\"predicted_playback_host_ns\":20"));
        assert!(text.contains("\"retrace_cycle\":200"));
        assert_eq!(sink.seal_once().unwrap(), None);
        let mut replacement =
            PresentationTraceSink::from_values(Some(path.as_os_str()), Some("replacement"), epoch)
                .unwrap();
        assert!(replacement
            .seal_once()
            .unwrap_err()
            .contains("refused output"));
    }
}
