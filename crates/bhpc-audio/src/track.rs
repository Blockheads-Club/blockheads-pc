//! Host model of `android.media.AudioTrack` in `MODE_STREAM`, for the fake JVM (`bhpc-jni`).
//!
//! libOpenAL's AudioTrack backend needs about seven members of that class: the constructor,
//! `getMinBufferSize`, `play`, `pause`, `stop`, `flush`, `release`, `write([BII)I` and
//! `getPlaybackHeadPosition`. Each is one method here, so the JNI layer only converts arguments.
//! `write` blocks while the track holds more than its buffer size, which is what paces OpenAL's
//! mixing thread.

use crate::mixer::{MixError, Mixer, PlayState, StreamId};
use crate::pcm::{PcmFormat, SampleFormat};
use std::time::Duration;

/// `AudioFormat.ENCODING_PCM_16BIT` / `_8BIT` / `_FLOAT`.
pub const ENCODING_PCM_16BIT: i32 = 2;
pub const ENCODING_PCM_8BIT: i32 = 3;
pub const ENCODING_PCM_FLOAT: i32 = 4;

/// Default for how long `write` waits for room before giving up on a stalled device.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// Buffers a track may hold at once (each `write` is one buffer).
const MAX_BUFFERS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackError {
    /// Unsupported sample rate, channel configuration or encoding (`ERROR_BAD_VALUE`).
    BadValue,
    /// The track was released (`ERROR_INVALID_OPERATION`).
    Released,
}

fn channels(config: i32) -> Option<u8> {
    match config {
        2 | 4 => Some(1),  // CHANNEL_CONFIGURATION_MONO, CHANNEL_OUT_MONO
        3 | 12 => Some(2), // CHANNEL_CONFIGURATION_STEREO, CHANNEL_OUT_STEREO
        _ => None,
    }
}

fn sample(encoding: i32) -> Option<SampleFormat> {
    match encoding {
        ENCODING_PCM_16BIT => Some(SampleFormat::S16),
        ENCODING_PCM_8BIT => Some(SampleFormat::U8),
        ENCODING_PCM_FLOAT => Some(SampleFormat::F32),
        _ => None,
    }
}

fn format(rate_hz: i32, channel_config: i32, encoding: i32) -> Result<PcmFormat, TrackError> {
    match (u32::try_from(rate_hz), channels(channel_config), sample(encoding)) {
        (Ok(r), Some(channels), Some(sample)) if (4000..=192_000).contains(&r) => {
            Ok(PcmFormat { rate_hz: r, channels, sample })
        }
        _ => Err(TrackError::BadValue),
    }
}

/// `AudioTrack.getMinBufferSize`: about 40 ms of audio. `None` stands for `ERROR_BAD_VALUE`.
pub fn min_buffer_size(rate_hz: i32, channel_config: i32, encoding: i32) -> Option<i32> {
    let f = format(rate_hz, channel_config, encoding).ok()?;
    Some((f.rate_hz / 25 * f.frame_bytes() as u32) as i32)
}

/// A destroyed mixer stream means the track is gone.
fn mix<T>(r: Result<T, MixError>) -> Result<T, TrackError> {
    r.map_err(|_| TrackError::Released)
}

/// One playing (or not) stream of PCM.
pub struct AudioTrack {
    mixer: Mixer,
    id: StreamId,
    fmt: PcmFormat,
    capacity_frames: usize,
    write_timeout: Duration,
    released: bool,
}

impl AudioTrack {
    /// `new AudioTrack(STREAM_MUSIC, rate, channelConfig, encoding, bufferBytes, MODE_STREAM)`.
    pub fn new(
        mixer: &Mixer,
        rate_hz: i32,
        channel_config: i32,
        encoding: i32,
        buffer_bytes: i32,
    ) -> Result<AudioTrack, TrackError> {
        let fmt = format(rate_hz, channel_config, encoding)?;
        let capacity_frames = (buffer_bytes.max(0) as usize / fmt.frame_bytes()).max(1);
        let id = mixer.create_stream(fmt.rate_hz, MAX_BUFFERS);
        Ok(AudioTrack { mixer: mixer.clone(), id, fmt, capacity_frames, write_timeout: WRITE_TIMEOUT, released: false })
    }

    /// How long `write` waits for room (default one second).
    pub fn set_write_timeout(&mut self, t: Duration) {
        self.write_timeout = t;
    }

    pub fn format(&self) -> PcmFormat {
        self.fmt
    }

    fn live(&self) -> Result<(), TrackError> {
        if self.released {
            Err(TrackError::Released)
        } else {
            Ok(())
        }
    }

    /// The mixer stream behind this track.
    pub fn stream_id(&self) -> StreamId {
        self.id
    }

    fn set(&self, state: PlayState) -> Result<(), TrackError> {
        self.live()?;
        mix(self.mixer.set_state(self.id, state))
    }

    pub fn play(&self) -> Result<(), TrackError> {
        self.set(PlayState::Playing)
    }

    pub fn pause(&self) -> Result<(), TrackError> {
        self.set(PlayState::Paused)
    }

    /// Stops and discards the queued data (Android would first play it out).
    pub fn stop(&self) -> Result<(), TrackError> {
        self.set(PlayState::Stopped)?;
        self.flush()
    }

    pub fn flush(&self) -> Result<(), TrackError> {
        self.live()?;
        mix(self.mixer.clear(self.id))
    }

    /// `AudioTrack.setStereoVolume` / `setVolume` (linear).
    pub fn set_volume(&self, gain: f32) -> Result<(), TrackError> {
        self.live()?;
        mix(self.mixer.set_gain(self.id, gain))
    }

    /// Frames played since the last `stop`.
    pub fn playback_head_position(&self) -> Result<u32, TrackError> {
        self.live()?;
        mix(self.mixer.played_frames(self.id)).map(|f| f as u32)
    }

    /// `write(byte[], offset, size)`: queue whole frames, waiting for room if the track is full.
    /// Returns the number of bytes consumed (0 if the device stalled for a second; a trailing
    /// partial frame is dropped and counted as consumed).
    pub fn write(&self, data: &[u8]) -> Result<usize, TrackError> {
        self.live()?;
        let frames = self.fmt.decode(data);
        if frames.is_empty() {
            return Ok(data.len());
        }
        let room = self.capacity_frames.saturating_sub(frames.len());
        if !self.mixer.wait_for_space(self.id, room, self.write_timeout) {
            return Ok(0);
        }
        // A full queue (too many tiny writes since the mixer last ran) is reported as a stall.
        Ok(self.mixer.enqueue(self.id, frames).map_or(0, |()| data.len()))
    }

    /// `release()`: the track can no longer be used.
    pub fn release(&mut self) {
        if !self.released {
            self.released = true;
            self.mixer.destroy_stream(self.id);
        }
    }
}

impl Drop for AudioTrack {
    fn drop(&mut self) {
        self.release();
    }
}
