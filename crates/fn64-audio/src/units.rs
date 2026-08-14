use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};

macro_rules! nonzero_rate {
    ($name:ident, $domain:literal) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(NonZeroU32);

        impl $name {
            pub const fn new(value: u32) -> Self {
                let Some(value) = NonZeroU32::new(value) else {
                    panic!(concat!($domain, " sample rate must be nonzero"));
                };
                Self(value)
            }

            pub const fn get(self) -> u32 {
                self.0.get()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.get().fmt(formatter)
            }
        }
    };
}

nonzero_rate!(GuestSampleRateHz, "guest");
nonzero_rate!(HostSampleRateHz, "host");

/// Number of interleaved channels in one PCM frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelCount(NonZeroU16);

impl ChannelCount {
    pub const MONO: Self = Self::new(1);
    pub const STEREO: Self = Self::new(2);

    pub const fn new(value: u16) -> Self {
        let Some(value) = NonZeroU16::new(value) else {
            panic!("audio channel count must be nonzero");
        };
        Self(value)
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }

    pub const fn as_usize(self) -> usize {
        self.get() as usize
    }
}

impl fmt::Display for ChannelCount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

macro_rules! count_type {
    ($name:ident, $storage:ty) => {
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name($storage);

        impl $name {
            pub const ZERO: Self = Self(0);

            pub const fn new(value: $storage) -> Self {
                Self(value)
            }

            pub const fn get(self) -> $storage {
                self.0
            }

            pub const fn saturating_sub(self, earlier: Self) -> Self {
                Self(self.0.saturating_sub(earlier.0))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.get().fmt(formatter)
            }
        }
    };
}

count_type!(GuestSampleSlotCount, u64);
count_type!(HostSampleSlotCount, u64);
count_type!(HostFrameCount, u64);
count_type!(GuestDmaByteCount, u32);

/// One guest-produced interleaved signed-16 PCM buffer.
///
/// Construction proves that the slice contains complete frames for its
/// channel count. The backend therefore cannot mistake a scalar sample-slot
/// count for a frame count or silently truncate a partial frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GuestPcm16<'audio> {
    samples: &'audio [i16],
    channels: ChannelCount,
}

impl<'audio> GuestPcm16<'audio> {
    pub fn new(samples: &'audio [i16], channels: ChannelCount) -> Self {
        assert_eq!(
            samples.len() % channels.as_usize(),
            0,
            "guest PCM sample slots must contain complete interleaved frames"
        );
        Self { samples, channels }
    }

    pub const fn samples(self) -> &'audio [i16] {
        self.samples
    }

    pub const fn channels(self) -> ChannelCount {
        self.channels
    }

    pub fn sample_slots(self) -> GuestSampleSlotCount {
        GuestSampleSlotCount::new(
            u64::try_from(self.samples.len()).expect("guest PCM length must fit u64"),
        )
    }

    pub fn dma_bytes(self) -> GuestDmaByteCount {
        let bytes = self
            .samples
            .len()
            .checked_mul(std::mem::size_of::<i16>())
            .and_then(|bytes| u32::try_from(bytes).ok())
            .expect("guest PCM byte length must fit the AI DMA register");
        GuestDmaByteCount::new(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_pcm_binds_channels_slots_and_dma_bytes() {
        let samples = [1, 2, 3, 4, 5, 6];
        let pcm = GuestPcm16::new(&samples, ChannelCount::STEREO);
        assert_eq!(pcm.channels(), ChannelCount::STEREO);
        assert_eq!(pcm.sample_slots(), GuestSampleSlotCount::new(6));
        assert_eq!(pcm.dma_bytes(), GuestDmaByteCount::new(12));
        assert_eq!(pcm.samples(), &samples);
    }

    #[test]
    #[should_panic(expected = "complete interleaved frames")]
    fn guest_pcm_rejects_partial_frames() {
        let _ = GuestPcm16::new(&[1, 2, 3], ChannelCount::STEREO);
    }

    #[test]
    #[should_panic(expected = "guest sample rate must be nonzero")]
    fn guest_rate_rejects_zero() {
        let _ = GuestSampleRateHz::new(0);
    }

    #[test]
    #[should_panic(expected = "host sample rate must be nonzero")]
    fn host_rate_rejects_zero() {
        let _ = HostSampleRateHz::new(0);
    }

    #[test]
    #[should_panic(expected = "channel count must be nonzero")]
    fn channel_count_rejects_zero() {
        let _ = ChannelCount::new(0);
    }
}
