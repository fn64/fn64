//! Stateful Voice Recognition Unit (VRU) dictionary and result seam.
//!
//! Public `os_voice.h` defines handle/data layouts and command states; the
//! public Voice Recognition System manuals define dictionary sizing, masking,
//! gain ranges, and the start/stop/result lifecycle. Recognition itself is a
//! host input: a host backend injects a result and guest polling never
//! fabricates a spoken word.

pub const VOICE_STATUS_READY: u8 = 0;
pub const VOICE_STATUS_START: u8 = 1;
pub const VOICE_STATUS_CANCEL: u8 = 3;
pub const VOICE_STATUS_BUSY: u8 = 5;
pub const VOICE_STATUS_END: u8 = 7;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum VoiceStatus {
    #[default]
    Ready = VOICE_STATUS_READY,
    Start = VOICE_STATUS_START,
    Cancel = VOICE_STATUS_CANCEL,
    Busy = VOICE_STATUS_BUSY,
    End = VOICE_STATUS_END,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoiceData {
    pub warning: u16,
    pub answer_num: u16,
    pub voice_level: u16,
    pub voice_sn: u16,
    pub voice_time: u16,
    pub answer: [u16; 5],
    pub distance: [u16; 5],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceError {
    Invalid,
    InvalidWord,
    NotReady,
}

#[derive(Clone, Debug)]
pub struct VoiceUnit {
    initialized: bool,
    expected_words: Option<u8>,
    words: Vec<Vec<u8>>,
    mask: Vec<u8>,
    analog_gain: u8,
    digital_gain: u8,
    status: VoiceStatus,
    pending_result: Option<VoiceData>,
}

impl Default for VoiceUnit {
    fn default() -> Self {
        Self {
            initialized: false,
            expected_words: None,
            words: Vec::new(),
            mask: Vec::new(),
            analog_gain: 0,
            digital_gain: 0,
            status: VoiceStatus::Ready,
            pending_result: None,
        }
    }
}

impl VoiceUnit {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn initialize(&mut self) {
        self.initialized = true;
        self.status = VoiceStatus::Ready;
        self.pending_result = None;
    }

    pub fn initialized(&self) -> bool {
        self.initialized
    }

    pub fn status(&self) -> u8 {
        self.status as u8
    }

    pub fn clear_dictionary(&mut self, words: u8) -> Result<(), VoiceError> {
        if !self.initialized {
            return Err(VoiceError::Invalid);
        }
        if words == 0 {
            return Err(VoiceError::Invalid);
        }
        self.expected_words = Some(words);
        self.words.clear();
        self.mask = vec![0xFF; usize::from(words).div_ceil(8)];
        self.pending_result = None;
        self.status = VoiceStatus::Ready;
        Ok(())
    }

    pub fn set_word(&mut self, word: &[u8]) -> Result<(), VoiceError> {
        if !self.initialized {
            return Err(VoiceError::Invalid);
        }
        if word.is_empty() || word.len() > 34 {
            return Err(VoiceError::InvalidWord);
        }
        let expected = self.expected_words.ok_or(VoiceError::Invalid)?;
        if self.words.len() >= usize::from(expected) {
            return Err(VoiceError::Invalid);
        }
        self.words.push(word.to_vec());
        Ok(())
    }

    pub fn set_mask(&mut self, mask: &[u8]) -> Result<(), VoiceError> {
        if !self.initialized {
            return Err(VoiceError::Invalid);
        }
        let expected = self.expected_words.ok_or(VoiceError::Invalid)?;
        if mask.len() != usize::from(expected).div_ceil(8) {
            return Err(VoiceError::Invalid);
        }
        self.mask.clear();
        self.mask.extend_from_slice(mask);
        Ok(())
    }

    pub fn set_gain(&mut self, analog: i32, digital: i32) -> Result<(), VoiceError> {
        if !self.initialized {
            return Err(VoiceError::Invalid);
        }
        if !(0..=1).contains(&analog) || !(0..=7).contains(&digital) {
            return Err(VoiceError::Invalid);
        }
        self.analog_gain = analog as u8;
        self.digital_gain = digital as u8;
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), VoiceError> {
        if !self.initialized {
            return Err(VoiceError::Invalid);
        }
        let expected = self.expected_words.ok_or(VoiceError::Invalid)?;
        if self.words.len() != usize::from(expected) {
            return Err(VoiceError::Invalid);
        }
        self.pending_result = None;
        self.status = VoiceStatus::Start;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.pending_result = None;
        self.status = VoiceStatus::Cancel;
    }

    pub fn mark_voice_detected(&mut self) -> Result<(), VoiceError> {
        match self.status {
            VoiceStatus::Start | VoiceStatus::Cancel => {
                self.status = VoiceStatus::Busy;
                Ok(())
            }
            VoiceStatus::Ready | VoiceStatus::Busy | VoiceStatus::End => Err(VoiceError::NotReady),
        }
    }

    pub fn inject_result(&mut self, mut result: VoiceData) -> Result<(), VoiceError> {
        if !matches!(
            self.status,
            VoiceStatus::Start | VoiceStatus::Cancel | VoiceStatus::Busy
        ) {
            return Err(VoiceError::NotReady);
        }
        result.answer_num = result.answer_num.min(5);
        self.pending_result = Some(result);
        self.status = VoiceStatus::End;
        Ok(())
    }

    pub fn take_result(&mut self) -> Result<VoiceData, VoiceError> {
        let result = self.pending_result.take().ok_or(VoiceError::NotReady)?;
        self.status = VoiceStatus::Ready;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognition_waits_for_host_result_instead_of_fabricating_one() {
        let mut voice = VoiceUnit::new();
        voice.initialize();
        voice.clear_dictionary(1).unwrap();
        voice.set_word(b"test").unwrap();
        voice.start().unwrap();
        assert_eq!(voice.take_result(), Err(VoiceError::NotReady));
        voice.mark_voice_detected().unwrap();
        assert_eq!(voice.status(), VOICE_STATUS_BUSY);
        voice
            .inject_result(VoiceData {
                answer_num: 1,
                answer: [0, 0x7FFF, 0x7FFF, 0x7FFF, 0x7FFF],
                ..VoiceData::default()
            })
            .unwrap();
        assert_eq!(voice.take_result().unwrap().answer_num, 1);
        assert_eq!(voice.status(), VOICE_STATUS_READY);
    }

    #[test]
    fn initialization_and_recognition_transitions_are_explicit() {
        let mut voice = VoiceUnit::new();
        assert!(!voice.initialized());
        assert_eq!(voice.clear_dictionary(1), Err(VoiceError::Invalid));
        assert_eq!(voice.mark_voice_detected(), Err(VoiceError::NotReady));

        voice.initialize();
        assert!(voice.initialized());
        voice.clear_dictionary(1).unwrap();
        voice.set_word(b"test").unwrap();
        voice.start().unwrap();
        voice.mark_voice_detected().unwrap();
        assert_eq!(voice.status(), VOICE_STATUS_BUSY);
        assert_eq!(voice.mark_voice_detected(), Err(VoiceError::NotReady));
    }
}
