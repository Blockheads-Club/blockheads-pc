//! Voice chat playback: one 48 kHz stream per speaker on the shared [`Mixer`], fed with decoded mono PCM that gets
//! the speaker's gain and a constant-power stereo pan baked in.
//!
//! Decoding, jitter buffering, VAD and the volume policy live in the voice core (mod-sdk `bhmod-voice`); this is only
//! the output seam it pushes 20 ms frames into. Streams are *quiet* (no [`crate::MixerEvent`]s: nothing guest-side
//! waits for them) and start playing at once; a speaker who stops talking simply underruns into silence.

use crate::mixer::{MixError, Mixer, PlayState, StreamId};
use crate::pcm::Frame;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// The voice sample rate (Opus' native rate).
pub const VOICE_RATE: u32 = 48_000;
/// Samples of one 20 ms frame at [`VOICE_RATE`].
pub const FRAME_SAMPLES: usize = 960;
/// Buffers (frames) a speaker's stream holds at most: 25 x 20 ms = 0.5 s. The jitter buffer upstream keeps it far
/// below that; a full stream refuses more ([`MixError::QueueFull`]) instead of growing the delay.
pub const MAX_BUFFERS: usize = 25;
/// Largest linear gain accepted (+12 dB); the client's 0-200 % volumes times the server's 0-200 % stay below it.
pub const MAX_GAIN: f32 = 4.0;

/// A speaker's stream, from [`VoiceOut::voice_stream`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceHandle {
    pub speaker: u32,
    stream: StreamId,
}

/// Left and right gains of a constant-power pan: `pan` -1 (left) ..= 1 (right), 0 = centre (both -3 dB).
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let p = if pan.is_nan() { 0.0 } else { pan.clamp(-1.0, 1.0) };
    let theta = (p + 1.0) * std::f32::consts::FRAC_PI_4;
    (theta.cos(), theta.sin())
}

/// Mono PCM to stereo frames with `gain` (linear, clamped to `0..=MAX_GAIN`) and `pan` applied; saturates.
pub fn spatialise(pcm: &[i16], gain: f32, pan: f32) -> Vec<Frame> {
    let g = if gain.is_nan() { 0.0 } else { gain.clamp(0.0, MAX_GAIN) };
    let (l, r) = pan_gains(pan);
    let (l, r) = (l * g, r * g);
    let sat = |v: f32| v.clamp(-32768.0, 32767.0) as i16;
    pcm.iter().map(|&s| [sat(s as f32 * l), sat(s as f32 * r)]).collect()
}

/// The voice streams of every speaker heard right now.
pub struct VoiceOut {
    mixer: Mixer,
    streams: Mutex<HashMap<u32, StreamId>>,
}

impl VoiceOut {
    /// Voice output on `mixer` (the game's: voice is summed into the same device).
    pub fn new(mixer: Mixer) -> VoiceOut {
        VoiceOut { mixer, streams: Mutex::new(HashMap::new()) }
    }

    /// The stream of `speaker`, made (playing, quiet) on first use.
    pub fn voice_stream(&self, speaker: u32) -> VoiceHandle {
        let mut s = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
        let stream = *s.entry(speaker).or_insert_with(|| {
            let id = self.mixer.create_stream(VOICE_RATE, MAX_BUFFERS);
            let _ = self.mixer.set_quiet(id, true);
            let _ = self.mixer.set_state(id, PlayState::Playing);
            id
        });
        VoiceHandle { speaker, stream }
    }

    /// Queue one decoded mono frame (any length; normally [`FRAME_SAMPLES`]) of the handle's speaker.
    pub fn push(&self, h: VoiceHandle, pcm_mono: &[i16], gain: f32, pan: f32) -> Result<(), MixError> {
        self.mixer.enqueue(h.stream, spatialise(pcm_mono, gain, pan))
    }

    /// Queue interleaved stereo samples (-1..1, the voice client's finished mix) on the handle's stream; saturates.
    pub fn push_stereo(&self, h: VoiceHandle, stereo: &[f32]) -> Result<(), MixError> {
        let sat = |v: f32| (v * 32768.0).clamp(-32768.0, 32767.0) as i16;
        self.mixer.enqueue(h.stream, stereo.chunks_exact(2).map(|p| [sat(p[0]), sat(p[1])]).collect())
    }

    /// Source frames of `h` not played yet (for the jitter buffer's latency estimate).
    pub fn queued(&self, h: VoiceHandle) -> Result<usize, MixError> {
        self.mixer.queued_frames(h.stream)
    }

    /// The speaker left (or was muted for good): its stream goes away with anything still queued. False if it had
    /// none.
    pub fn drop_speaker(&self, speaker: u32) -> bool {
        let id = self.streams.lock().unwrap_or_else(PoisonError::into_inner).remove(&speaker);
        id.is_some_and(|id| self.mixer.destroy_stream(id))
    }

    /// Every stream goes away (the session ended).
    pub fn clear(&self) {
        let all: Vec<_> =
            self.streams.lock().unwrap_or_else(PoisonError::into_inner).drain().map(|(_, id)| id).collect();
        for id in all {
            self.mixer.destroy_stream(id);
        }
    }

    /// Speakers with a stream.
    pub fn speakers(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.streams.lock().unwrap_or_else(PoisonError::into_inner).keys().copied().collect();
        v.sort_unstable();
        v
    }
}

impl Drop for VoiceOut {
    fn drop(&mut self) {
        self.clear();
    }
}
