//! Half duplex. The mic is drained every pull; playback overflow is dropped.
//! There is no acoustic echo canceller and no second device stack.

use std::sync::atomic::Ordering;
#[cfg(test)]
use std::sync::{Arc, Mutex};

use zeron_audio::dsp::{Resampler, level};
use zeron_audio::native::AudioIo;
use zeron_proto::voice::VoiceRejection;

pub trait Duplex: Send {
    fn open(&mut self) -> Result<(), VoiceRejection>;
    fn set_muted(&mut self, muted: bool);
    /// Append newly captured mic audio as 24 kHz mono f32. Drain the device
    /// ring every call so it cannot overflow. The caller discards the samples
    /// when muted or while the speaker is playing.
    fn pull_mic(&mut self, out: &mut Vec<f32>) -> Result<(), VoiceRejection>;
    fn push_speaker(&mut self, pcm: &[i16]);
    fn levels(&mut self) -> (u16, u16);
    fn speaker_queued_ms(&self) -> u32;
    fn close(&mut self);
}

#[derive(Default)]
pub struct DesktopDuplex {
    io: Option<AudioIo>,
    resampler: Option<Resampler>,
    mic_level: u16,
    speaker_level: u16,
    sequence: u64,
}

impl Duplex for DesktopDuplex {
    fn open(&mut self) -> Result<(), VoiceRejection> {
        self.close();
        let io = AudioIo::open(1).map_err(|_| VoiceRejection::DeviceUnavailable)?;
        if io.start().is_err() {
            return Err(VoiceRejection::DeviceUnavailable);
        }
        let resampler =
            Resampler::new(io.input_rate, 24_000).ok_or(VoiceRejection::DeviceUnavailable)?;
        self.io = Some(io);
        self.resampler = Some(resampler);
        self.sequence = 0;
        self.mic_level = 0;
        self.speaker_level = 0;
        Ok(())
    }

    fn set_muted(&mut self, muted: bool) {
        // `AudioIo::mute` drains the capture ring on every call, including unmute.
        if let Some(io) = self.io.as_ref() {
            if io.muted.load(Ordering::Acquire) != muted {
                io.mute(muted);
            }
        }
        if muted {
            self.mic_level = 0;
        }
    }

    fn pull_mic(&mut self, out: &mut Vec<f32>) -> Result<(), VoiceRejection> {
        let raw = {
            let Some(io) = self.io.as_ref() else {
                return Err(VoiceRejection::DeviceUnavailable);
            };
            // The output callback also fills `reference`. Leaving it full sets
            // the device failed flag. There is no AEC; drain and drop.
            drain_reference(io);
            let mut raw = Vec::new();
            while let Some(sample) = io.capture.pop() {
                raw.push(sample);
            }
            if io.failed.load(Ordering::Acquire) {
                return Err(VoiceRejection::DeviceUnavailable);
            }
            raw
        };
        let start = out.len();
        {
            let Some(resampler) = self.resampler.as_mut() else {
                return Err(VoiceRejection::DeviceUnavailable);
            };
            for sample in raw {
                resampler.push(sample, out);
            }
        }
        self.mic_level = level_u16(&out[start..]);
        Ok(())
    }

    fn push_speaker(&mut self, pcm: &[i16]) {
        if pcm.is_empty() || self.io.is_none() {
            return;
        }
        let samples: Vec<f32> = pcm.iter().map(|sample| *sample as f32 / 32_768.0).collect();
        self.speaker_level = level_u16(&samples);
        let mut sequence = self.sequence;
        if let Some(io) = self.io.as_mut() {
            drain_reference(io);
            for chunk in samples.chunks(480) {
                sequence = sequence.wrapping_add(1);
                let Ok(frame) = zeron_audio::encode(chunk, 1, sequence) else {
                    continue;
                };
                if io.enqueue(&frame).is_err() {
                    break;
                }
            }
        }
        self.sequence = sequence;
    }

    fn levels(&mut self) -> (u16, u16) {
        let queued = self.speaker_queued_ms();
        let muted = self
            .io
            .as_ref()
            .is_some_and(|io| io.muted.load(Ordering::Acquire));
        let mic = if muted { 0 } else { self.mic_level };
        let speaker = if queued == 0 { 0 } else { self.speaker_level };
        (mic, speaker)
    }

    fn speaker_queued_ms(&self) -> u32 {
        let Some(io) = self.io.as_ref() else {
            return 0;
        };
        drain_reference(io);
        let rate = u64::from(io.output_rate.max(1));
        let ms = io.output_queued() as u64 * 1_000 / rate;
        u32::try_from(ms).unwrap_or(u32::MAX)
    }

    fn close(&mut self) {
        self.io = None;
        self.resampler = None;
        self.mic_level = 0;
        self.speaker_level = 0;
    }
}

fn drain_reference(io: &AudioIo) {
    while io.reference.pop().is_some() {}
}

fn level_u16(samples: &[f32]) -> u16 {
    (level(samples) * u16::MAX as f32) as u16
}

/// Records speaker PCM and reports an empty playback queue.
#[cfg(test)]
pub(crate) struct NullDuplex {
    pub pcm: Arc<Mutex<Vec<i16>>>,
    muted: bool,
}

#[cfg(test)]
impl Default for NullDuplex {
    fn default() -> Self {
        Self {
            pcm: Arc::new(Mutex::new(Vec::new())),
            muted: false,
        }
    }
}

#[cfg(test)]
impl Duplex for NullDuplex {
    fn open(&mut self) -> Result<(), VoiceRejection> {
        Ok(())
    }

    fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    fn pull_mic(&mut self, _out: &mut Vec<f32>) -> Result<(), VoiceRejection> {
        Ok(())
    }

    fn push_speaker(&mut self, pcm: &[i16]) {
        self.pcm.lock().expect("pcm").extend_from_slice(pcm);
    }

    fn levels(&mut self) -> (u16, u16) {
        let _ = self.muted;
        (0, 0)
    }

    fn speaker_queued_ms(&self) -> u32 {
        0
    }

    fn close(&mut self) {}
}
