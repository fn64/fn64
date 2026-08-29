//! Opt-in correlation trace for host audio and video presentation.
//!
//! This stream is deliberately separate from `fn64-timing-trace`: device
//! events are deterministic guest evidence, while callback and window
//! timestamps are host observations. The trace is bounded in memory and
//! sealed with `create_new` during clean exit.
//!
//! `FN64_AV_SYNC_CUE_ID` adds one opaque, externally defined cue identity to
//! the trace. It requires both existing exact probes and emits an explicit pair
//! only while the selected output sample's continuity generation remains live;
//! the shell never assigns title semantics or pairs nearest timestamps itself.

use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const TRACE_PATH_ENV: &str = "FN64_PRESENTATION_TRACE";
const TRACE_ID_ENV: &str = "FN64_PRESENTATION_TRACE_ID";
const CUE_ID_ENV: &str = "FN64_AV_SYNC_CUE_ID";
const MAX_RECORDS: usize = 100_000;

#[derive(Debug)]
struct Config {
    path: PathBuf,
    cue_id: Option<String>,
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
        let cue_id = std::env::var(CUE_ID_ENV).ok();
        if cue_id.is_some() {
            for required in ["FN64_AV_SYNC_PROBE", "FN64_AV_SYNC_VIDEO_HASH"] {
                if std::env::var_os(required).is_none() {
                    return Err(format!("{CUE_ID_ENV} requires {required}"));
                }
            }
        }
        Self::from_values(
            path.as_deref(),
            trace_id.as_deref(),
            cue_id.as_deref(),
            std::time::Instant::now(),
        )
    }

    fn from_values(
        path: Option<&std::ffi::OsStr>,
        trace_id: Option<&str>,
        cue_id: Option<&str>,
        epoch: std::time::Instant,
    ) -> Result<Self, String> {
        let Some(path) = path else {
            if trace_id.is_some() {
                return Err(format!("{TRACE_ID_ENV} requires {TRACE_PATH_ENV}"));
            }
            if cue_id.is_some() {
                return Err(format!("{CUE_ID_ENV} requires {TRACE_PATH_ENV}"));
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
        validate_id(TRACE_ID_ENV, trace_id)?;
        let cue_id = cue_id
            .map(|raw| {
                let value = raw.trim();
                if value.is_empty() {
                    return Err(format!("{CUE_ID_ENV} must be nonempty when it is set"));
                }
                validate_id(CUE_ID_ENV, value)?;
                Ok::<_, String>(value.to_owned())
            })
            .transpose()?;
        let cue_id_json = cue_id
            .as_ref()
            .map(|value| format!("\"{value}\""))
            .unwrap_or_else(|| "null".to_owned());
        Ok(Self {
            config: Some(Config { path, cue_id }),
            epoch: Some(epoch),
            records: vec![format!(
                "{{\"record\":\"header\",\"schema\":\"fn64.host-presentation.v4\",\"trace_id\":\"{trace_id}\",\"cue_id\":{cue_id_json},\"host_time\":\"nanoseconds_from_trace_epoch\",\"emulated_time\":\"r4300_master_cycle\",\"emulated_hz\":{}}}",
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

    pub fn record_audio_cue(
        &mut self,
        landmark: fn64_audio::AudioSyncLandmark,
        state: Option<fn64_audio::AudioPresentationState>,
        observed_at: std::time::Instant,
    ) {
        let Some(cue_id) = self.cue_id() else {
            return;
        };
        let observed_ns = self.relative_ns(observed_at);
        let playback_ns = landmark
            .predicted_playback_at
            .map(|instant| self.relative_ns(instant));
        let current_generation = state.map(|value| value.continuity_generation);
        let invalid_reason = audio_cue_invalid_reason(landmark, state);
        let valid = invalid_reason.is_none();
        let invalid_reason = invalid_reason
            .map(|reason| format!("\"{reason}\""))
            .unwrap_or_else(|| "null".to_owned());
        self.push(format!(
            "{{\"record\":\"av_cue_audio\",\"cue_id\":\"{cue_id}\",\"dma_id\":{},\"guest_frame_offset\":{},\"dma_start_cycle\":{},\"start_dacrate\":{},\"ai_clock_hz\":{},\"predicted_playback_host_ns\":{},\"landmark_generation\":{},\"current_generation\":{},\"dropped\":{},\"retimed\":{},\"valid\":{valid},\"invalid_reason\":{invalid_reason},\"observed_host_ns\":{observed_ns}}}",
            landmark.dma_id.get(),
            landmark.guest_frame_offset,
            json_option(landmark.dma_started_at.map(fn64_runtime::Cycles::get)),
            json_option(landmark.start_dacrate),
            fn64_abi::vi_clock_hz(),
            json_option(playback_ns),
            json_option(landmark.continuity_generation),
            json_option(current_generation),
            landmark.dropped_before_playback,
            landmark.retimed_after_start,
        ));
    }

    pub fn record_video_cue(&mut self, landmark: crate::timing::VideoSyncLandmark) {
        let Some(cue_id) = self.cue_id() else {
            return;
        };
        let presented_ns = self.relative_ns(landmark.presented_at);
        self.push(format!(
            "{{\"record\":\"av_cue_video\",\"cue_id\":\"{cue_id}\",\"rgba_hash\":\"{:016x}\",\"occurrence\":{},\"stage\":\"{}\",\"presentation_generation\":{},\"swap_count\":{},\"retrace_cycle\":{},\"present_return_host_ns\":{presented_ns}}}",
            landmark.rgba_hash,
            landmark.occurrence,
            landmark.stage.serialized_name(),
            landmark.presentation_generation,
            landmark.swap_count,
            landmark.retrace_at.get(),
        ));
    }

    pub fn record_av_cue_pair(
        &mut self,
        audio: fn64_audio::AudioSyncLandmark,
        video: crate::timing::VideoSyncLandmark,
        state: Option<fn64_audio::AudioPresentationState>,
    ) -> bool {
        let Some(cue_id) = self.cue_id() else {
            return false;
        };
        if audio_cue_invalid_reason(audio, state).is_some() {
            return false;
        }
        let start = audio
            .dma_started_at
            .expect("validated audio cue has DMA start");
        let dacrate = audio
            .start_dacrate
            .expect("validated audio cue has DAC rate");
        let playback = audio
            .predicted_playback_at
            .expect("validated audio cue has playback time");
        let generation = audio
            .continuity_generation
            .expect("validated audio cue has continuity generation");
        let denominator = u128::from(fn64_abi::vi_clock_hz());
        let dac_divisor = dacrate
            .checked_add(1)
            .expect("AI DAC rate divisor overflowed");
        let audio_cycle_numerator = u128::from(start.get()) * denominator
            + u128::from(audio.guest_frame_offset)
                * u128::from(fn64_runtime::CPU_CLOCK_HZ)
                * u128::from(dac_divisor);
        let video_cycle_numerator = u128::from(video.retrace_at.get()) * denominator;
        let guest_phase_numerator = i128::try_from(video_cycle_numerator)
            .expect("video cue cycle numerator exceeds i128")
            - i128::try_from(audio_cycle_numerator)
                .expect("audio cue cycle numerator exceeds i128");
        let audio_host_ns = self.relative_ns(playback);
        let video_host_ns = self.relative_ns(video.presented_at);
        let host_phase_ns = video_host_ns - audio_host_ns;
        self.push(format!(
            "{{\"record\":\"av_cue_pair\",\"cue_id\":\"{cue_id}\",\"audio_dma_id\":{},\"audio_guest_frame_offset\":{},\"audio_generation\":{generation},\"audio_cycle_numerator\":{audio_cycle_numerator},\"video_hash\":\"{:016x}\",\"video_occurrence\":{},\"video_stage\":\"{}\",\"video_presentation_generation\":{},\"video_retrace_cycle\":{},\"cycle_denominator\":{denominator},\"video_minus_audio_guest_numerator\":{guest_phase_numerator},\"audio_predicted_playback_host_ns\":{audio_host_ns},\"video_present_return_host_ns\":{video_host_ns},\"video_minus_audio_host_ns\":{host_phase_ns}}}",
            audio.dma_id.get(),
            audio.guest_frame_offset,
            video.rgba_hash,
            video.occurrence,
            video.stage.serialized_name(),
            video.presentation_generation,
            video.retrace_at.get(),
        ));
        true
    }

    pub fn record_render_batches(
        &mut self,
        observations: impl IntoIterator<Item = fn64_abi::RenderBatchObservation>,
    ) {
        if !self.is_enabled() {
            return;
        }
        for observation in observations {
            let execution_mode = match observation.execution_mode {
                fn64_abi::RenderBatchExecutionMode::Worker => "worker",
                fn64_abi::RenderBatchExecutionMode::Local => "local",
            };
            let worker_start_ns = observation
                .worker
                .map(|span| self.relative_ns(span.started_at).to_string())
                .unwrap_or_else(|| "null".to_string());
            let worker_finish_ns = observation
                .worker
                .map(|span| self.relative_ns(span.finished_at).to_string())
                .unwrap_or_else(|| "null".to_string());
            let join_cause = match observation.join.map(|join| join.cause) {
                Some(fn64_abi::RenderBatchJoinCause::ViVisibility) => "\"vi_visibility\"",
                Some(fn64_abi::RenderBatchJoinCause::LaterGraphics) => "\"later_graphics\"",
                Some(fn64_abi::RenderBatchJoinCause::DmemDependency) => "\"dmem_dependency\"",
                Some(fn64_abi::RenderBatchJoinCause::LaterGraphicsAndDmemDependency) => {
                    "\"later_graphics_and_dmem_dependency\""
                }
                None => "null",
            };
            let join_request_ns = observation
                .join
                .map(|span| self.relative_ns(span.requested_at).to_string())
                .unwrap_or_else(|| "null".to_string());
            let join_return_ns = observation
                .join
                .map(|span| self.relative_ns(span.returned_at).to_string())
                .unwrap_or_else(|| "null".to_string());
            let dispatch_ns = self.relative_ns(observation.dispatch_host_at);
            self.push(format!(
                "{{\"record\":\"render_batch\",\"batch_id\":{},\"members\":{},\"execution_mode\":\"{execution_mode}\",\"dispatch_cycle\":{},\"completion_cycle\":{},\"dispatch_host_ns\":{dispatch_ns},\"worker_start_host_ns\":{worker_start_ns},\"worker_finish_host_ns\":{worker_finish_ns},\"join_cause\":{join_cause},\"join_request_host_ns\":{join_request_ns},\"join_return_host_ns\":{join_return_ns},\"staged_writes_ns\":{},\"commit_ns\":{},\"copyback_ns\":{},\"publication_ns\":{}}}",
                observation.batch_id,
                observation.member_count,
                observation.dispatch_cycle.get(),
                observation.completion_cycle.get(),
                observation.staged_writes.as_nanos(),
                observation.commit.as_nanos(),
                observation.copyback.as_nanos(),
                observation.publication.as_nanos(),
            ));
        }
    }

    pub fn record_render_batch_incomplete(
        &mut self,
        observation: fn64_abi::RenderBatchIncompleteObservation,
    ) {
        if !self.is_enabled() {
            return;
        }
        let reason = match observation.reason {
            fn64_abi::RenderBatchIncompleteReason::ProcessExitBeforeCompletion => {
                "process_exit_before_completion"
            }
        };
        let dispatch_ns = self.relative_ns(observation.dispatch_host_at);
        self.push(format!(
            "{{\"record\":\"render_batch_incomplete\",\"batch_id\":{},\"members\":{},\"dispatch_cycle\":{},\"dispatch_host_ns\":{dispatch_ns},\"reason\":\"{reason}\"}}",
            observation.batch_id,
            observation.member_count,
            observation.dispatch_cycle.get(),
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

    fn cue_id(&self) -> Option<String> {
        self.config
            .as_ref()
            .and_then(|config| config.cue_id.clone())
    }

    fn push(&mut self, record: String) {
        assert!(
            self.records.len() < MAX_RECORDS,
            "host presentation trace exceeded its {MAX_RECORDS}-record bound"
        );
        self.records.push(record);
    }
}

fn validate_id(env: &str, value: &str) -> Result<(), String> {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._-:".contains(&byte))
    {
        Ok(())
    } else {
        Err(format!(
            "{env} may contain only ASCII letters, digits, '.', '_', '-', and ':'"
        ))
    }
}

fn json_option<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned())
}

fn audio_cue_invalid_reason(
    landmark: fn64_audio::AudioSyncLandmark,
    state: Option<fn64_audio::AudioPresentationState>,
) -> Option<&'static str> {
    if landmark.dropped_before_playback {
        return Some("dropped_before_playback");
    }
    if landmark.retimed_after_start {
        return Some("retimed_after_start");
    }
    if landmark.dma_started_at.is_none() {
        return Some("missing_dma_start");
    }
    if landmark.start_dacrate.is_none() {
        return Some("missing_dacrate");
    }
    if landmark.predicted_playback_at.is_none() {
        return Some("missing_predicted_playback");
    }
    let Some(landmark_generation) = landmark.continuity_generation else {
        return Some("missing_landmark_generation");
    };
    let Some(state) = state else {
        return Some("missing_continuity_state");
    };
    (state.continuity_generation != landmark_generation).then_some("continuity_generation_changed")
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
        let mut disabled = PresentationTraceSink::from_values(None, None, None, epoch).unwrap();
        assert!(!disabled.is_enabled());
        assert_eq!(disabled.seal_once().unwrap(), None);
        assert!(
            PresentationTraceSink::from_values(
                Some("relative".as_ref()),
                Some("run"),
                None,
                epoch,
            )
                .unwrap_err()
                .contains("absolute")
        );
        assert!(
            PresentationTraceSink::from_values(None, Some("run"), None, epoch)
                .unwrap_err()
                .contains(TRACE_PATH_ENV)
        );
        assert!(
            PresentationTraceSink::from_values(None, None, Some("cue"), epoch)
                .unwrap_err()
                .contains(TRACE_PATH_ENV)
        );
        let path = std::env::temp_dir().join("fn64-unused-presentation-trace.jsonl");
        assert!(
            PresentationTraceSink::from_values(
                Some(path.as_os_str()),
                Some("run"),
                Some("  "),
                epoch,
            )
            .unwrap_err()
            .contains("must be nonempty")
        );
        assert!(
            PresentationTraceSink::from_values(
                Some(path.as_os_str()),
                Some("run"),
                Some("not/a/cue"),
                epoch,
            )
            .unwrap_err()
            .contains("ASCII letters")
        );
    }

    #[test]
    fn trace_joins_generation_anchor_and_exact_vi_present() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("presentation.jsonl");
        let epoch = std::time::Instant::now();
        let mut sink = PresentationTraceSink::from_values(
            Some(path.as_os_str()),
            Some("joined-run"),
            None,
            epoch,
        )
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
        assert!(text.contains("\"schema\":\"fn64.host-presentation.v4\""));
        assert!(text.contains(
            "\"record\":\"vi_present\",\"stage\":\"post_vi\",\"presentation_generation\":9"
        ));
        assert!(text.contains("\"predicted_playback_host_ns\":20"));
        assert!(text.contains("\"retrace_cycle\":200"));
        assert_eq!(sink.seal_once().unwrap(), None);
        let mut replacement = PresentationTraceSink::from_values(
            Some(path.as_os_str()),
            Some("replacement"),
            None,
            epoch,
        )
        .unwrap();
        assert!(
            replacement
                .seal_once()
                .unwrap_err()
                .contains("refused output")
        );
    }

    #[test]
    fn exact_cue_pair_requires_matching_audio_continuity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("exact-cue.jsonl");
        let epoch = std::time::Instant::now();
        let mut sink = PresentationTraceSink::from_values(
            Some(path.as_os_str()),
            Some("exact-cue"),
            Some("cue-1"),
            epoch,
        )
        .unwrap();
        let audio = fn64_audio::AudioSyncLandmark {
            dma_id: fn64_runtime::AiDmaId::new(7),
            guest_frame_offset: 3,
            dma_started_at: Some(fn64_runtime::Cycles::new(100)),
            start_dacrate: Some(1_519),
            predicted_playback_at: Some(epoch + std::time::Duration::from_nanos(200)),
            continuity_generation: Some(4),
            dropped_before_playback: false,
            retimed_after_start: false,
        };
        let valid_state = fn64_audio::AudioPresentationState {
            continuity_generation: 4,
            anchor: None,
        };
        let video = crate::timing::VideoSyncLandmark {
            rgba_hash: 0x1234,
            occurrence: std::num::NonZeroU64::new(2).unwrap(),
            stage: fn64_abi::PresentedViFieldStage::PostVi,
            presentation_generation: 9,
            swap_count: 11,
            retrace_at: fn64_runtime::EmulatedInstant::new(300),
            presented_at: epoch + std::time::Duration::from_nanos(250),
        };
        sink.record_audio_cue(
            audio,
            Some(valid_state),
            epoch + std::time::Duration::from_nanos(210),
        );
        sink.record_video_cue(video);
        assert!(sink.record_av_cue_pair(audio, video, Some(valid_state)));

        let changed_state = fn64_audio::AudioPresentationState {
            continuity_generation: 5,
            anchor: None,
        };
        assert!(!sink.record_av_cue_pair(audio, video, Some(changed_state)));
        let receipt = sink.seal_once().unwrap().unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert_eq!(receipt.records, 5);
        assert!(text.contains("\"cue_id\":\"cue-1\""));
        assert!(text.contains("\"record\":\"av_cue_audio\""));
        assert!(text.contains("\"landmark_generation\":4,\"current_generation\":4"));
        assert!(text.contains("\"record\":\"av_cue_video\""));
        assert_eq!(text.matches("\"record\":\"av_cue_pair\"").count(), 1);
        assert!(text.contains("\"video_minus_audio_host_ns\":50"));
    }

    #[test]
    fn exact_audio_cue_records_invalidity_without_emitting_a_pair() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid-cue.jsonl");
        let epoch = std::time::Instant::now();
        let mut sink = PresentationTraceSink::from_values(
            Some(path.as_os_str()),
            Some("invalid-cue"),
            Some("cue-2"),
            epoch,
        )
        .unwrap();
        let audio = fn64_audio::AudioSyncLandmark {
            dma_id: fn64_runtime::AiDmaId::new(8),
            guest_frame_offset: 0,
            dma_started_at: Some(fn64_runtime::Cycles::new(100)),
            start_dacrate: Some(1_519),
            predicted_playback_at: Some(epoch + std::time::Duration::from_nanos(200)),
            continuity_generation: Some(4),
            dropped_before_playback: false,
            retimed_after_start: false,
        };
        let changed = fn64_audio::AudioPresentationState {
            continuity_generation: 5,
            anchor: None,
        };
        sink.record_audio_cue(audio, Some(changed), epoch);
        sink.seal_once().unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("\"valid\":false"));
        assert!(text.contains("\"invalid_reason\":\"continuity_generation_changed\""));
        assert!(!text.contains("\"record\":\"av_cue_pair\""));
    }

    #[test]
    fn render_batch_schema_covers_local_worker_and_every_join_cause() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("render-batches.jsonl");
        let epoch = std::time::Instant::now();
        let mut sink = PresentationTraceSink::from_values(
            Some(path.as_os_str()),
            Some("render-batches"),
            None,
            epoch,
        )
        .unwrap();
        let causes = [
            (
                fn64_abi::RenderBatchJoinCause::ViVisibility,
                "vi_visibility",
            ),
            (
                fn64_abi::RenderBatchJoinCause::LaterGraphics,
                "later_graphics",
            ),
            (
                fn64_abi::RenderBatchJoinCause::DmemDependency,
                "dmem_dependency",
            ),
            (
                fn64_abi::RenderBatchJoinCause::LaterGraphicsAndDmemDependency,
                "later_graphics_and_dmem_dependency",
            ),
        ];
        let worker = causes.iter().enumerate().map(|(index, (cause, _))| {
            let start = epoch + std::time::Duration::from_nanos(10 + index as u64 * 10);
            fn64_abi::RenderBatchObservation {
                batch_id: index as u64,
                member_count: index + 1,
                dispatch_cycle: fn64_runtime::EmulatedInstant::new(index as u64),
                completion_cycle: fn64_runtime::EmulatedInstant::new(index as u64 + 1),
                dispatch_host_at: start,
                execution_mode: fn64_abi::RenderBatchExecutionMode::Worker,
                worker: Some(fn64_abi::RenderWorkerSpan {
                    started_at: start,
                    finished_at: start + std::time::Duration::from_nanos(2),
                }),
                join: Some(fn64_abi::RenderBatchJoinSpan {
                    cause: *cause,
                    requested_at: start + std::time::Duration::from_nanos(1),
                    returned_at: start + std::time::Duration::from_nanos(3),
                }),
                staged_writes: std::time::Duration::ZERO,
                commit: std::time::Duration::ZERO,
                copyback: std::time::Duration::ZERO,
                publication: std::time::Duration::ZERO,
            }
        });
        let local = fn64_abi::RenderBatchObservation {
            batch_id: 4,
            member_count: 1,
            dispatch_cycle: fn64_runtime::EmulatedInstant::new(4),
            completion_cycle: fn64_runtime::EmulatedInstant::new(5),
            dispatch_host_at: epoch + std::time::Duration::from_nanos(50),
            execution_mode: fn64_abi::RenderBatchExecutionMode::Local,
            worker: None,
            join: None,
            staged_writes: std::time::Duration::ZERO,
            commit: std::time::Duration::ZERO,
            copyback: std::time::Duration::ZERO,
            publication: std::time::Duration::ZERO,
        };
        sink.record_vi_present(
            fn64_abi::PresentedViFieldStage::PostVi,
            17,
            fn64_runtime::EmulatedInstant::new(6),
            3,
            0x1234,
            320,
            240,
            epoch + std::time::Duration::from_nanos(60),
        );
        sink.record_render_batches(worker.chain(std::iter::once(local)));
        sink.record_render_batch_incomplete(fn64_abi::RenderBatchIncompleteObservation {
            batch_id: 5,
            member_count: 2,
            dispatch_cycle: fn64_runtime::EmulatedInstant::new(6),
            dispatch_host_at: epoch + std::time::Duration::from_nanos(55),
            reason: fn64_abi::RenderBatchIncompleteReason::ProcessExitBeforeCompletion,
        });
        let receipt = sink.seal_once().unwrap().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(receipt.records, 9);
        assert!(text.contains("\"schema\":\"fn64.host-presentation.v4\""));
        assert!(text.contains(
            "\"record\":\"vi_present\",\"stage\":\"post_vi\",\"presentation_generation\":17"
        ));
        assert_eq!(text.matches("\"execution_mode\":\"worker\"").count(), 4);
        assert_eq!(text.matches("\"execution_mode\":\"local\"").count(), 1);
        for (_, cause) in causes {
            assert!(text.contains(&format!("\"join_cause\":\"{cause}\"")));
        }
        assert!(text.contains("\"execution_mode\":\"local\",\"dispatch_cycle\":4"));
        assert!(text.contains("\"worker_start_host_ns\":null"));
        assert!(text.contains("\"join_cause\":null"));
        assert!(text.contains(
            "\"record\":\"render_batch_incomplete\",\"batch_id\":5,\"members\":2,\"dispatch_cycle\":6,\"dispatch_host_ns\":55,\"reason\":\"process_exit_before_completion\""
        ));
    }
}
