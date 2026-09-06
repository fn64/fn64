//! Console television-standard clocks shared by VI and AI.
//!
//! Provenance: public `rcp.h` defines `VI_NTSC_CLOCK`, `VI_PAL_CLOCK`, and
//! `VI_MPAL_CLOCK`, and defines AI's sample rate as `video_clock / dac_period`.
//! The VI register contract defines H_SYNC as one line's terminal video-clock
//! count and V_SYNC as the terminal half-line count per field. The effective
//! counts are each one greater than the stored register, so the field
//! conversion below uses `(h_sync + 1) * (v_sync + 1) / 2` video clocks. See
//! N64 Timing Reference section 5.1.1, derived from U.S. Patent 6,331,856
//! sheets 46--47. The public VI manual identifies NTSC/MPAL as nominal 60 Hz
//! and PAL as nominal 50 Hz; that nominal rate is used only until a nonzero
//! register pair is present.

/// R4300 guest CPU clock used by deterministic device deadlines.
pub const CPU_CLOCK_HZ: u64 = 93_750_000;

/// IPL-selected television standard stored in the public `osTvType` global.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum TvType {
    Pal = 0,
    #[default]
    Ntsc = 1,
    Mpal = 2,
}

impl TvType {
    pub const fn vi_clock_hz(self) -> u32 {
        match self {
            Self::Pal => 49_656_530,
            Self::Ntsc => 48_681_812,
            Self::Mpal => 48_628_316,
        }
    }

    pub const fn nominal_field_hz(self) -> u32 {
        match self {
            Self::Pal => 50,
            Self::Ntsc | Self::Mpal => 60,
        }
    }

    pub const fn nominal_field_cycles(self) -> u64 {
        CPU_CLOCK_HZ.div_ceil(self.nominal_field_hz() as u64)
    }

    /// CPU cycles occupied by one programmed VI field. Returns `None` until
    /// both timing registers are nonzero, allowing boot to use the public
    /// nominal field rate before the VI manager installs a mode.
    pub fn programmed_field_cycles(self, h_sync: u32, v_sync: u32) -> Option<u64> {
        let h_sync_terminal = h_sync & 0x0fff;
        let v_sync_terminal = v_sync & 0x03ff;
        if h_sync_terminal == 0 || v_sync_terminal == 0 {
            return None;
        }
        let h_sync = u128::from(h_sync_terminal + 1);
        let v_sync = u128::from(v_sync_terminal + 1);
        let numerator = u128::from(CPU_CLOCK_HZ) * h_sync * v_sync;
        let denominator = u128::from(self.vi_clock_hz()) * 2;
        Some(
            u64::try_from(numerator.div_ceil(denominator))
                .expect("programmed VI field duration exceeds u64"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_video_clocks_and_nominal_rates_are_region_specific() {
        assert_eq!(TvType::Ntsc.vi_clock_hz(), 48_681_812);
        assert_eq!(TvType::Pal.vi_clock_hz(), 49_656_530);
        assert_eq!(TvType::Mpal.vi_clock_hz(), 48_628_316);
        assert_eq!(TvType::Ntsc.nominal_field_cycles(), 1_562_500);
        assert_eq!(TvType::Pal.nominal_field_cycles(), 1_875_000);
        assert_eq!(TvType::Mpal.nominal_field_cycles(), 1_562_500);
    }

    #[test]
    fn programmed_ntsc_timing_expands_terminal_hsync_and_half_line_counts() {
        let cycles = TvType::Ntsc.programmed_field_cycles(3_093, 525).unwrap();
        assert_eq!(cycles, 1_567_042);
        assert!(TvType::Ntsc.programmed_field_cycles(0, 525).is_none());
        assert!(TvType::Ntsc.programmed_field_cycles(3_093, 0).is_none());
    }
}
