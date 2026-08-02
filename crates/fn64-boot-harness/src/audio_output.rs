use fn64_audio::{AudioBackend as _, AudioConfig, AudioError, CpalBackend};
use std::time::{Duration, Instant};

/// Result of connecting a headless boot harness's AI boundary to host audio.
#[derive(Debug)]
pub enum LiveAudioOutput {
    Disabled,
    Active {
        guest_rate_hz: u32,
        stream_rate_hz: u32,
    },
    Unavailable(AudioError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveAudioPaceError {
    BackendUnavailable,
    TimedOut {
        buffered_frames: u32,
        target_frames: u32,
    },
}

impl std::fmt::Display for LiveAudioPaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BackendUnavailable => {
                write!(f, "live audio pacing requires an active output backend")
            }
            Self::TimedOut {
                buffered_frames,
                target_frames,
            } => write!(
                f,
                "live audio pacing timed out with {buffered_frames} frames buffered; target is at most {target_frames}"
            ),
        }
    }
}

impl std::error::Error for LiveAudioPaceError {}

/// Register the harness RDRAM bounds and optionally open the default host
/// output stream. The bounds and PCM diagnostics remain active when playback
/// is disabled or the device cannot be opened.
pub fn wire_live_audio_output(
    rdram_len: usize,
    guest_rate_hz: u32,
    disabled: bool,
) -> LiveAudioOutput {
    fn64_abi::set_audio_rdram_len(rdram_len);
    if disabled {
        return LiveAudioOutput::Disabled;
    }

    let mut backend = CpalBackend::new();
    match backend.create(&AudioConfig::new(guest_rate_hz, 2)) {
        Ok(()) => {
            let stream_rate_hz = backend.stream_rate_hz().unwrap_or(guest_rate_hz);
            fn64_abi::set_audio_backend(Box::new(backend), rdram_len);
            LiveAudioOutput::Active {
                guest_rate_hz,
                stream_rate_hz,
            }
        }
        Err(error) => LiveAudioOutput::Unavailable(error),
    }
}

/// Wait for the host callback to reduce its private output queue. This is a
/// harness wall-clock control only; it does not advance or mutate guest time.
pub fn pace_live_audio_output(
    target_frames: u32,
    timeout: Duration,
) -> Result<(), LiveAudioPaceError> {
    let started = Instant::now();
    loop {
        let buffered_frames =
            fn64_abi::audio_frames_remaining().ok_or(LiveAudioPaceError::BackendUnavailable)?;
        if buffered_frames <= target_frames {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err(LiveAudioPaceError::TimedOut {
                buffered_frames,
                target_frames,
            });
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_output_registers_diagnostics_without_opening_a_device() {
        assert!(matches!(
            wire_live_audio_output(8 * 1024 * 1024, 32_000, true),
            LiveAudioOutput::Disabled
        ));
        assert_eq!(
            pace_live_audio_output(0, Duration::ZERO),
            Err(LiveAudioPaceError::BackendUnavailable)
        );
    }
}
